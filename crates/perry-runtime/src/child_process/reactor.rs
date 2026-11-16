//! Async subprocess reactor for `child_process.spawn` — #1934.
//!
//! The buffered `spawn` (pre-#1934) ran the child to completion synchronously
//! and replayed a single `data` chunk + `exit`/`close` on the next tick. That
//! is observationally correct for short commands but precludes a *live* child:
//! `stdin.write()` had nothing to write to, `kill()` had nothing to signal, and
//! stdout arrived in one post-exit lump.
//!
//! This reactor launches the child without blocking and wires three background
//! threads per child:
//!   * a stdout reader and a stderr reader that read pipe chunks and push them
//!     onto [`CP_EVENT_QUEUE`], and
//!   * a waiter that blocks on `Child::wait()` and pushes the exit status.
//! Each producer calls [`js_notify_main_thread`] so the event loop wakes
//! immediately. The main-thread [`cp_reactor_pump`] (driven from
//! `js_run_stdlib_pump`, which both the end-of-main event loop and the `await`
//! poll loop call) drains the queue, builds `Buffer`s, and emits `spawn` /
//! `data` / `end` / `exit` / `close` on the ChildProcess. Live children keep
//! the event loop alive via [`cp_reactor_has_live`] (wired into
//! `js_stdlib_has_active_handles`).
//!
//! Background threads never touch JSValues (those are thread-local); they move
//! only raw bytes / exit codes across the boundary. The ChildProcess object is
//! kept reachable across ticks by [`cp_reactor_scan_roots_mut`], a registered
//! GC mutable-root scanner.

use std::collections::HashMap;
use std::io::BufRead;
use std::io::{Read, Write};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

// Brings in the `cp_*` helpers, `CpFn`, `CP_SHAPE_ID`, the `TAG_*` consts, and
// (since this is a descendant module) `mod.rs`'s `use` bindings such as
// `ClosureHeader` / `JSValue`.
use super::*;

type IpcStream = std::os::unix::net::UnixStream;

type CpWaiter = Box<dyn FnOnce() -> (Option<i32>, Option<i32>) + Send>;

// The pre-socket stdio path: thread/loop readers and the stdin writer. On unix
// every pipe slot is a net.Socket over a socketpair (`pipe_sockets`).
mod exec_async;
mod pipe_sockets;
pub(super) use exec_async::{cp_defer_exec_callback, cp_exec_async};
use exec_async::{cp_exec_accumulate, cp_exec_fire_close};
pub(super) use pipe_sockets::*;

/// Monotonic registry key for live children.
static CP_NEXT_LIVE_ID: AtomicU64 = AtomicU64::new(1);

/// Number of live (spawned, not-yet-`close`d) children. A lock-free fast-path
/// gate: the pump and the active-handle check both bail on zero without taking
/// any lock, so the hot async loop pays a single relaxed load per tick.
static CP_LIVE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Live children which currently keep the event loop alive. `unref()` only
/// changes this count; the total live count above continues to gate pumping
/// and GC scanning while the runtime is alive for another reason.
static CP_REFED_COUNT: AtomicU64 = AtomicU64::new(0);

/// An event produced by a child's background thread, consumed by the pump.
enum CpEvent {
    /// The child process terminated (`code` xor `signal`).
    Exited {
        handle: u64,
        code: Option<i32>,
        signal: Option<i32>,
    },
    /// #1933: one IPC line (`process.send` on the child side) — raw JSON to be
    /// parsed + delivered as a `'message'` event on the main thread.
    Message { handle: u64, json: String },
    /// #2130: one IPC frame under `serialization: 'advanced'` — the raw V8
    /// structured-clone payload (header included, length prefix stripped) to be
    /// deserialized + delivered as a `'message'` event on the main thread.
    MessageAdvanced { handle: u64, bytes: Vec<u8> },
    /// #1933: the IPC channel closed (child disconnected or exited) — flip
    /// `connected`/`channel` and emit `'disconnect'`.
    IpcClosed { handle: u64 },
    /// Timeout expired; terminate the live child with this signal on the main
    /// thread so JS-visible `killed` is updated with the same event ordering.
    Timeout { handle: u64, signal: i32 },
    /// `AbortSignal` attached through `options.signal` fired.
    Abort { handle: u64 },
}

static CP_EVENT_QUEUE: Mutex<Vec<CpEvent>> = Mutex::new(Vec::new());

/// Per-child reactor state. The `Child` itself is owned by the waiter thread
/// (so reaping blocks off the main thread); here we keep only what the main
/// thread needs: the GC-rooted ChildProcess value, the live `pid` for
/// `kill()`, the writable `stdin`, and progress flags.
struct LiveChild {
    /// The agent (main thread or worker) that spawned the child. Its
    /// ChildProcess and stream objects live on that agent's heap and its
    /// listeners are that agent's closures, so only that agent's pump emits
    /// this child's events, roots its values and counts it as a loop handle.
    /// The registry and the event queue are process-global; every other
    /// agent's pump leaves this entry and its queued events untouched.
    owner: crate::agent::AgentId,
    /// NaN-boxed ChildProcess object — a GC root (see `cp_reactor_scan_roots_mut`).
    cp_bits: u64,
    process_ids: crate::async_hook_slots::AsyncResourceIds,
    pipe_ids: [crate::async_hook_slots::AsyncResourceIds; 3],
    pipe_bits: [u64; 3],
    pid: i32,
    /// Whether the `spawn` event has been emitted yet.
    spawned: bool,
    /// `Some((code, signal))` once the waiter reported termination.
    exited: Option<(Option<i32>, Option<i32>)>,
    /// Whether `exit` has been emitted. `close` follows on the next pump.
    exit_emitted: bool,
    /// Node's `maybeClose` counters (mirrored on the ChildProcess as
    /// `_closesNeeded`/`_closesGot`): 1 for the exit, 1 per readable pipe,
    /// 1 for the IPC channel. `close` is emitted once they are equal.
    closes_needed: u32,
    closes_got: u32,
    /// Whether `close` has been emitted (terminal state).
    closed: bool,
    /// Whether this process currently contributes an active event-loop handle.
    refed: bool,
    /// #1933: the parent end of the IPC socket for a `fork()`ed child (a clone
    /// for `child.send()` / `child.disconnect()`; the reader thread owns
    /// another clone). `None` for plain `spawn`.
    ipc_send: Option<IpcStream>,
    /// #2130: whether the IPC channel uses V8 structured-clone framing
    /// (`serialization: 'advanced'`) rather than newline-delimited JSON.
    ipc_advanced: bool,
    /// NaN-boxed AbortSignal value for listener cleanup/GC rooting.
    abort_signal_bits: u64,
    /// NaN-boxed abort-listener closure value for listener cleanup/GC rooting.
    abort_listener_bits: u64,
    /// Signal number used when `options.signal` aborts this child.
    abort_kill_signal: i32,
    /// #4912: present for children launched by the async `exec`/`execFile`
    /// callback form. When set, the pump buffers stdout/stderr instead of
    /// emitting stream events and fires this single `(err, stdout, stderr)`
    /// callback on `close` — Node's "run off-thread, call back on a later
    /// tick" model. `None` for `spawn`/`fork`.
    exec: Option<Box<CpExecPending>>,
}


/// Buffered state for an async `exec`/`execFile` child (#4912). Output is
/// accumulated off the main thread and replayed into the same `CpRun`-shaped
/// callback the synchronous path used, so the deferred callback is
/// byte-identical to the former immediate one.
pub(super) struct CpExecPending {
    /// NaN-boxed callback closure — a GC root until the callback fires.
    cb_bits: u64,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// `maxBuffer` / `timeout` / `killSignal` limits + the error shape source.
    run_options: CpRunOptions,
    /// utf8/buffer encoding for the boxed stdout/stderr.
    mode: CpOutput,
    /// Human-readable command, for the error `.cmd` field.
    cmd: String,
    /// Program actually launched, for the spawn-failure `syscall`/`path`.
    file: String,
    /// Set once accumulated output passed `maxBuffer` (the child is then killed).
    exceeded: bool,
    /// Set when the `timeout` fired and the child was killed.
    timed_out: bool,
}

static CP_LIVE: Mutex<Option<HashMap<u64, LiveChild>>> = Mutex::new(None);

fn cp_init_async_resources(
    cp: f64,
    stdin_obj: Option<f64>,
    stdout_obj: f64,
    stderr_obj: f64,
) -> (
    crate::async_hook_slots::AsyncResourceIds,
    [crate::async_hook_slots::AsyncResourceIds; 3],
) {
    let process_ids = crate::async_hook_slots::init_resource("PROCESSWRAP", cp, true);
    // A slot that is not a pipe (`null` on unix) has no pipe resource.
    let pipe = |object: Option<f64>| match object {
        Some(object) if cp_object_ptr(object).is_some() => {
            crate::async_hook_slots::init_resource("PIPEWRAP", object, true)
        }
        _ => crate::async_hook_slots::AsyncResourceIds {
            async_id: 0,
            trigger_async_id: 0,
        },
    };
    let pipe_ids = [
        pipe(stdin_obj),
        pipe(Some(stdout_obj)),
        pipe(Some(stderr_obj)),
    ];
    (process_ids, pipe_ids)
}

thread_local! {
    /// Re-entrancy guard — an emitted handler may itself drive the event loop
    /// (`await`), which re-enters `js_run_stdlib_pump` → `cp_reactor_pump`.
    static CP_PUMPING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[inline]
/// Register [`release_cp_children_in_freed_ranges`] before the first
/// `CP_LIVE` insert (#11541). Every insert calls this first; named in
/// `arena::thread_exit`'s dispatcher instead, the release (and `LiveChild`'s
/// drop glue) was linked into every binary.
fn register_thread_exit_release() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        crate::arena::thread_exit::register_thread_exit_range_hook(
            release_cp_children_in_freed_ranges,
        );
    });
}

fn cp_live_lock() -> std::sync::MutexGuard<'static, Option<HashMap<u64, LiveChild>>> {
    CP_LIVE.lock().unwrap_or_else(PoisonError::into_inner)
}

#[inline]
fn cp_queue_lock() -> std::sync::MutexGuard<'static, Vec<CpEvent>> {
    CP_EVENT_QUEUE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn cp_push_event(ev: CpEvent) {
    cp_queue_lock().push(ev);
    crate::event_pump::js_notify_main_thread();
}

impl CpEvent {
    fn handle(&self) -> u64 {
        match self {
            CpEvent::Exited { handle, .. }
            | CpEvent::Message { handle, .. }
            | CpEvent::MessageAdvanced { handle, .. }
            | CpEvent::IpcClosed { handle }
            | CpEvent::Timeout { handle, .. }
            | CpEvent::Abort { handle } => *handle,
        }
    }
}

/// Take the queued events this agent's pump may deliver, in queue order: those
/// of children it spawned, plus events for children no longer registered
/// (which every pump drops). Events of a child another agent spawned stay in
/// the queue, in order, for that agent's pump — they are never taken out, so
/// the owner cannot miss them between a foreign take and a put-back.
fn cp_take_own_events() -> Vec<CpEvent> {
    let foreign: std::collections::HashSet<u64> = match cp_live_lock().as_ref() {
        Some(map) => map
            .iter()
            .filter(|(_, lc)| !crate::agent::owns(lc.owner))
            .map(|(h, _)| *h)
            .collect(),
        None => std::collections::HashSet::new(),
    };
    let mut queue = cp_queue_lock();
    if foreign.is_empty() {
        return std::mem::take(&mut *queue);
    }
    let (kept, own): (Vec<CpEvent>, Vec<CpEvent>) = std::mem::take(&mut *queue)
        .into_iter()
        .partition(|ev| foreign.contains(&ev.handle()));
    *queue = kept;
    own
}

#[inline]
fn libc_sigterm() -> i32 {
    {
        libc::SIGTERM
    }
}

/// IPC reader (#1933): read newline-delimited JSON from the parent socket and
/// push each line for main-thread parse + `'message'` delivery. For
/// `serialization: 'advanced'` (#2130) the framing is instead a 4-byte
/// big-endian length prefix followed by that many V8-serialized payload bytes
/// (Node's `parseChannelMessages` shape).
fn cp_spawn_ipc_reader(handle: u64, sock: IpcStream, advanced: bool) {
    if advanced {
        cp_spawn_ipc_reader_advanced(handle, sock);
        return;
    }
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(sock);
        for line in reader.lines() {
            match line {
                Ok(l) if !l.is_empty() => cp_push_event(CpEvent::Message { handle, json: l }),
                Ok(_) => {} // blank keep-alive line
                Err(_) => break,
            }
        }
        // Channel closed (child disconnected / exited).
        cp_push_event(CpEvent::IpcClosed { handle });
    });
}

/// Advanced (#2130) IPC reader: accumulate bytes and emit one
/// [`CpEvent::MessageAdvanced`] per `[u32 BE length][payload]` frame. Robust to
/// a length field or payload split across socket reads (the
/// `advanced-serialization-splitted-length-field` case).
fn cp_spawn_ipc_reader_advanced(handle: u64, mut sock: IpcStream) {
    std::thread::spawn(move || {
        let mut acc: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 8192];
        loop {
            let n = match sock.read(&mut chunk) {
                Ok(0) => break, // EOF
                Ok(n) => n,
                Err(_) => break,
            };
            acc.extend_from_slice(&chunk[..n]);
            // Drain every complete frame currently buffered.
            let mut consumed = 0;
            while acc.len() - consumed >= 4 {
                let len = u32::from_be_bytes([
                    acc[consumed],
                    acc[consumed + 1],
                    acc[consumed + 2],
                    acc[consumed + 3],
                ]) as usize;
                if acc.len() - consumed - 4 < len {
                    break; // payload not fully arrived yet
                }
                let start = consumed + 4;
                let bytes = acc[start..start + len].to_vec();
                cp_push_event(CpEvent::MessageAdvanced { handle, bytes });
                consumed = start + len;
            }
            if consumed > 0 {
                acc.drain(..consumed);
            }
        }
        cp_push_event(CpEvent::IpcClosed { handle });
    });
}

/// Register a freshly-spawned child: take its stdio pipes, insert the registry
/// entry (with the optional IPC socket for `fork`), start the reader/waiter (+
/// IPC reader) threads, and set `pid`/`__cpHandle` on the ChildProcess + stdio
/// sub-objects. Shared by `spawn` (#1934) and `fork` (#1933). Returns the
/// registry handle.
pub(super) fn cp_register_live_child(
    cp: f64,
    stdout_obj: f64,
    stderr_obj: f64,
    stdin_obj: f64,
    readable_sockets: u32,
    #[cfg_attr(unix, allow(unused_mut))] mut child: Child,
    ipc: Option<IpcStream>,
    ipc_advanced: bool,
    timeout: Option<Duration>,
    kill_signal: i32,
) -> u64 {
    let pid = child.id();
    // Duplicate the process handle BEFORE `child` moves to the waiter thread —
    // the kill paths act on it instead of the recyclable pid.

    // On unix every pipe slot is a socket the caller attached; the child has
    // no std pipes left to take.
    let waiter: CpWaiter = Box::new(move || match child.wait() {
        Ok(status) => {
            let signal = {
                use std::os::unix::process::ExitStatusExt;
                status.signal()
            };
            (status.code(), signal)
        }
        Err(_) => (Some(-1), None),
    });
    cp_register_live_child_parts(
        cp,
        stdout_obj,
        stderr_obj,
        stdin_obj,
        readable_sockets,
        pid,
        waiter,
        ipc,
        ipc_advanced,
        timeout,
        kill_signal,
    )
}


#[allow(clippy::too_many_arguments)]
fn cp_register_live_child_parts(
    cp: f64,
    stdout_obj: f64,
    stderr_obj: f64,
    stdin_obj: f64,
    readable_sockets: u32,
    pid: u32,
    waiter: CpWaiter,
    ipc: Option<IpcStream>,
    ipc_advanced: bool,
    timeout: Option<Duration>,
    kill_signal: i32,
) -> u64 {
    cp_set_field(cp, b"pid", pid as f64);
    let readers = 0;
    let closes_needed = 1 + readable_sockets + readers + ipc.is_some() as u32;
    cp_set_field(cp, b"_closesNeeded", closes_needed as f64);
    cp_set_field(cp, b"_closesGot", 0.0);

    let handle = CP_NEXT_LIVE_ID.fetch_add(1, Ordering::SeqCst);
    let handle_f = handle as f64;
    cp_set_field(cp, b"__cpHandle", handle_f);
    // The reader/writer stream objects find their child by this key; a pipe
    // socket (unix) is the socket's own and carries none.
    let (process_ids, pipe_ids) =
        cp_init_async_resources(cp, Some(stdin_obj), stdout_obj, stderr_obj);

    // For fork, keep a clone of the IPC socket for send/disconnect; the reader
    // thread owns the original.
    let ipc_send = ipc.as_ref().and_then(|s| s.try_clone().ok());

    {
        register_thread_exit_release();
        let mut guard = cp_live_lock();
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(
            handle,
            LiveChild {
                owner: crate::agent::current_agent(),
                cp_bits: cp.to_bits(),
                process_ids,
                pipe_ids,
                pipe_bits: [
                    stdin_obj.to_bits(),
                    stdout_obj.to_bits(),
                    stderr_obj.to_bits(),
                ],
                pid: pid as i32,
                spawned: false,
                exited: None,
                exit_emitted: false,
                closes_needed,
                closes_got: 0,
                closed: false,
                refed: true,
                ipc_send,
                ipc_advanced,
                abort_signal_bits: 0,
                abort_listener_bits: 0,
                abort_kill_signal: libc_sigterm(),
                exec: None,
            },
        );
    }
    crate::stdlib_pump::register_runtime_pump(0, cp_reactor_pump_extern);
    CP_LIVE_COUNT.fetch_add(1, Ordering::SeqCst);
    CP_REFED_COUNT.fetch_add(1, Ordering::SeqCst);

    cp_spawn_waiter(
        handle,
        waiter,
        timeout.map(|timeout| (timeout, kill_signal)),
    );
    {
        if let Some(sock) = ipc {
            cp_spawn_ipc_reader(handle, sock, ipc_advanced);
        }
    }
    // Wake the loop so 'spawn' fires on the next tick.
    crate::event_pump::js_notify_main_thread();
    handle
}

extern "C" fn cp_abort_listener(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
) -> f64 {
    let handle = js_closure_get_capture_ptr(closure, 0) as u64;
    cp_push_event(CpEvent::Abort { handle });
    cp_undefined()
}

pub(super) fn cp_install_abort_signal(handle: u64, signal: Option<f64>, opts_val: f64) {
    let Some(signal) = signal else {
        return;
    };
    let Some(signal_ptr) = crate::event_target_hook_slots::abort_signal_ptr_from_value(signal)
    else {
        return;
    };

    let listener = js_closure_alloc(crate::fn_info!(cp_abort_listener, 0; with_declared(0)), 1);
    crate::closure::closure_set_capture_word(listener, 0, handle as i64);
    let listener_val = cp_box_ptr(listener as *const u8);
    let kill_signal = cp_read_abort_kill_signal(opts_val);

    if let Some(map) = cp_live_lock().as_mut() {
        if let Some(lc) = map.get_mut(&handle) {
            lc.abort_signal_bits = signal.to_bits();
            lc.abort_listener_bits = listener_val.to_bits();
            lc.abort_kill_signal = kill_signal;
        }
    }

    if crate::event_target_hook_slots::js_abort_signal_is_aborted(signal_ptr) != 0 {
        cp_push_event(CpEvent::Abort { handle });
    } else {
        crate::event_target_hook_slots::js_abort_signal_add_listener(
            signal_ptr,
            cp_box_string("abort"),
            listener_val,
        );
    }
}

fn cp_read_abort_kill_signal(opts_val: f64) -> i32 {
    if cp_object_ptr(opts_val).is_none() {
        return libc_sigterm();
    }
    let signal = cp_get_field(opts_val, b"killSignal");
    if JSValue::from_bits(signal.to_bits()).is_undefined() {
        return libc_sigterm();
    }
    cp_signal_number_from_value(signal)
}

fn cp_signal_number_from_value(signal: f64) -> i32 {
    {
        cp_parse_signal(signal)
    }
}

fn cp_take_abort_fields(handle: u64) -> Option<(u64, i32, u64, u64)> {
    let mut guard = cp_live_lock();
    let lc = guard.as_mut()?.get_mut(&handle)?;
    let fields = (
        lc.cp_bits,
        lc.abort_kill_signal,
        lc.abort_signal_bits,
        lc.abort_listener_bits,
    );
    lc.abort_signal_bits = 0;
    lc.abort_listener_bits = 0;
    Some(fields)
}

fn cp_cleanup_abort_listener(signal_bits: u64, listener_bits: u64) {
    if signal_bits == 0 || listener_bits == 0 {
        return;
    }
    let signal = f64::from_bits(signal_bits);
    let Some(signal_ptr) = crate::event_target_hook_slots::abort_signal_ptr_from_value(signal)
    else {
        return;
    };
    crate::event_target_hook_slots::js_abort_signal_remove_listener(
        signal_ptr,
        cp_box_string("abort"),
        f64::from_bits(listener_bits),
    );
}

/// `child.send(message)` — serialize `message` to JSON (main thread) and write
/// it newline-delimited to the IPC socket. Returns whether the write
/// succeeded. #1933.
pub(super) fn cp_ipc_send(handle: u64, message: f64) -> bool {

    {
        // Determine the channel's framing without holding the lock across the
        // (potentially GC-triggering) serialization below.
        let advanced = {
            let guard = cp_live_lock();
            guard
                .as_ref()
                .and_then(|map| map.get(&handle))
                .map(|lc| lc.ipc_advanced)
                .unwrap_or(false)
        };

        let frame: Vec<u8> = if advanced {
            // #2130: 4-byte big-endian length prefix + V8 structured-clone payload.
            let payload = super::v8_serde::v8_serialize(message);
            let mut f = Vec::with_capacity(payload.len() + 4);
            f.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            f.extend_from_slice(&payload);
            f
        } else {
            let sh = unsafe { crate::json::js_json_stringify(message, 0) };
            if sh.is_null() {
                return false;
            }
            let mut line = unsafe {
                let len = (*sh).byte_len as usize;
                let data = (sh as *const u8).add(std::mem::size_of::<StringHeader>());
                std::slice::from_raw_parts(data, len).to_vec()
            };
            line.push(b'\n');
            line
        };

        let mut guard = cp_live_lock();
        if let Some(map) = guard.as_mut() {
            if let Some(lc) = map.get_mut(&handle) {
                if let Some(sock) = lc.ipc_send.as_mut() {
                    return sock.write_all(&frame).is_ok();
                }
            }
        }
        false
    }
}

/// Write an already-serialized JSON line on a child's IPC channel (newline
/// appended). Used by the cluster primary's `queryServerReply` (#4962); shares
/// the live-child lock so it never interleaves with `cp_ipc_send`.
pub fn cp_ipc_send_raw_json(handle: u64, json: &str) -> bool {
    {
        use std::io::Write;
        // Match the channel's selected framing. Cluster's query-server reply
        // is built as JSON in Rust, but an advanced channel still requires a
        // V8-serialized payload just like every user-visible IPC message.
        let advanced = {
            let guard = cp_live_lock();
            guard
                .as_ref()
                .and_then(|map| map.get(&handle))
                .map(|lc| lc.ipc_advanced)
                .unwrap_or(false)
        };
        let frame = if advanced {
            let sh = crate::string::js_string_from_bytes(json.as_ptr(), json.len() as u32);
            let message = f64::from_bits(unsafe { crate::json::js_json_parse(sh) }.bits());
            let payload = super::v8_serde::v8_serialize(message);
            let mut frame = Vec::with_capacity(payload.len() + 4);
            frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            frame.extend_from_slice(&payload);
            frame
        } else {
            let mut frame = Vec::with_capacity(json.len() + 1);
            frame.extend_from_slice(json.as_bytes());
            frame.push(b'\n');
            frame
        };
        let mut guard = cp_live_lock();
        if let Some(map) = guard.as_mut() {
            if let Some(lc) = map.get_mut(&handle) {
                if let Some(sock) = lc.ipc_send.as_mut() {
                    return sock.write_all(&frame).is_ok();
                }
            }
        }
        false
    }
}

/// Pass a connection fd to a child over its IPC socketpair via `SCM_RIGHTS`
/// (#4962 SCHED_RR). Holds the live-child lock so the framed send is atomic
/// against `cp_ipc_send`. Returns false (caller closes the fd) when the child
/// is gone.
pub fn cp_ipc_send_fd(handle: u64, key_id: u32, payload_fd: std::os::unix::io::RawFd) -> bool {
    use std::os::unix::io::AsRawFd;
    let guard = cp_live_lock();
    let Some(map) = guard.as_ref() else {
        return false;
    };
    let Some(lc) = map.get(&handle) else {
        return false;
    };
    let Some(sock) = lc.ipc_send.as_ref() else {
        return false;
    };
    crate::cluster_sched::send_fd_over(sock.as_raw_fd(), key_id, payload_fd)
}

/// `child.disconnect()` — shut the IPC socket down (the reader thread then sees
/// EOF and exits). Returns whether a channel was present. #1933.
pub(super) fn cp_ipc_disconnect(handle: u64) -> bool {

    {
        let mut guard = cp_live_lock();
        if let Some(map) = guard.as_mut() {
            if let Some(lc) = map.get_mut(&handle) {
                if let Some(sock) = lc.ipc_send.take() {
                    let _ = sock.shutdown(std::net::Shutdown::Both);
                    return true;
                }
            }
        }
        false
    }
}

/// `child_process.spawn(command[, args][, options])` — returns a live streaming
/// ChildProcess (#1934). `cmd_ptr`/`args_ptr` are raw (unboxed) `StringHeader`
/// / `ArrayHeader` pointers; `opts_ptr` is a raw heap pointer (or 0). The
/// object SHAPE matches the former buffered implementation (`pid`, `exitCode`,
/// `signalCode`, `killed`, `connected`, `spawnargs`, `spawnfile`, `stdout`,
/// `stderr`, `stdin`, `stdio`) plus the hidden `__cpHandle` registry key.
#[no_mangle]
pub extern "C" fn js_child_process_spawn_streams(
    cmd_ptr: i64,
    args_ptr: i64,
    opts_ptr: i64,
) -> f64 {
    cp_require_process_support();
    let (cmd_str, arg_strs) = unsafe {
        (
            cp_read_string_header(cmd_ptr),
            cp_read_arg_strings(args_ptr),
        )
    };
    validate::cp_validate_no_null_bytes("file", &cmd_str);
    unsafe { validate::cp_validate_raw_args(args_ptr) };

    // `opts_ptr` arrives as a raw (unboxed) heap pointer; re-box it so the
    // options helpers can read `cwd`/`env`/`shell`. Small values mean
    // no-options (codegen passes 0) — leave it undefined then.
    let opts_val = cp_options_from_raw_args(args_ptr, opts_ptr);
    let abort_signal = cp_read_abort_signal(opts_val);

    // stdout/stderr Readable + stdin Writable sub-objects (unix: pipe
    // sockets, built once the spawn succeeded).
    let stdio_count = cp_array_ptr(cp_get_field(opts_val, b"stdio"))
        .map(|arr| crate::array::js_array_length(arr) as usize)
        .unwrap_or(3)
        .max(3);
    let stdio_kinds = cp_read_stdio(opts_val, stdio_count);
    let timeout = cp_read_timeout(opts_val);
    let kill_signal = cp_read_kill_signal(opts_val);

    // spawnargs = [argv0 ?? command, ...args]
    let mut spawnargs = crate::array::js_array_alloc((arg_strs.len() + 1) as u32);
    let argv0 = cp_spawnargs_argv0(&cmd_str, opts_val);
    spawnargs = crate::array::js_array_push_f64(spawnargs, cp_box_string(&argv0));
    for a in &arg_strs {
        spawnargs = crate::array::js_array_push_f64(spawnargs, cp_box_string(a));
    }

    let cp_methods: [(&str, CpFn); 11] = [
        ("on", crate::fn_info!(cp_method_on, 2; with_declared(2))),
        ("once", crate::fn_info!(cp_method_on, 2; with_declared(2))),
        (
            "addListener",
            crate::fn_info!(cp_method_on, 2; with_declared(2)),
        ),
        (
            "prependListener",
            crate::fn_info!(cp_method_on, 2; with_declared(2)),
        ),
        (
            "removeListener",
            crate::fn_info!(cp_method_remove_listener, 2; with_declared(2)),
        ),
        (
            "off",
            crate::fn_info!(cp_method_remove_listener, 2; with_declared(2)),
        ),
        (
            "removeAllListeners",
            crate::fn_info!(cp_method_remove_all_listeners, 1; with_declared(1)),
        ),
        ("emit", crate::fn_info!(cp_method_emit, 2; with_declared(2))),
        ("kill", crate::fn_info!(cp_method_kill, 1; with_declared(1))),
        ("ref", crate::fn_info!(cp_method_ref, 0; with_declared(0))),
        (
            "unref",
            crate::fn_info!(cp_method_unref, 0; with_declared(0)),
        ),
    ];
    let cp_obj = cp_build_object(&cp_methods, CP_SHAPE_ID + cp_methods.len() as u32);
    let cp = cp_box_ptr(cp_obj as *const u8);
    cp_install_dispose(cp);


    cp_set_field(cp, b"exitCode", TAG_NULL_F64);
    cp_set_field(cp, b"signalCode", TAG_NULL_F64);
    cp_set_field(cp, b"killed", TAG_FALSE_F64);
    cp_set_field(cp, b"connected", TAG_FALSE_F64);
    cp_set_field(cp, b"spawnargs", cp_box_ptr(spawnargs as *const u8));
    cp_set_field(cp, b"spawnfile", cp_box_string(&cmd_str));

    // Build + launch the child (honoring `shell`/`cwd`/`env`), non-blocking.
    let mut command = cp_build_command(&cmd_str, &arg_strs, opts_val);
    // A failed launch keeps the pipes it made: Node wraps them anyway.
    let launch = cp_apply_live_stdio(&mut command, &stdio_kinds)
        .map_err(|error| (error, Vec::new()))
        .and_then(|pipes| {
            let launch = command.spawn();
            match launch {
                Ok(child) => Ok((child, pipes)),
                Err(error) => Err((error, pipes)),
            }
        });

    match launch {
        Ok((child, parents)) => {
            let sockets = cp_attach_pipe_sockets(cp, &stdio_kinds, parents);
            let handle = cp_register_live_child(
                cp,
                sockets.slot(1),
                sockets.slot(2),
                sockets.slot(0),
                sockets.readable.len() as u32,
                child,
                None,
                false,
                timeout,
                kill_signal,
            );
            cp_watch_pipe_closes(handle, &sockets);
            cp_install_abort_signal(handle, abort_signal, opts_val);
        }
        Err((e, pipes)) => {
            // Spawn failure (e.g. ENOENT): Node emits a single `error` event and
            // never `spawn`/`exit`. Defer it so a synchronously-registered
            // handler is present. The error carries Node's errno shape:
            // `code`/`errno`/`syscall`/`path`/`spawnargs`, message
            // `spawn <cmd> <CODE>`.
            cp_set_field(cp, b"pid", cp_undefined());
            let code = super::cp_io_error_code(&e);
            let syscall = format!("spawn {cmd_str}");
            let message = format!("{syscall} {code}");
            // Node's error `spawnargs` excludes argv0 (internally `slice(1)`
            // of the handle's spawnargs), unlike `child.spawnargs`.
            let mut err_args = crate::array::js_array_alloc(arg_strs.len() as u32);
            for a in &arg_strs {
                err_args = crate::array::js_array_push_f64(err_args, cp_box_string(a));
            }
            let err = super::cp_make_error(
                &message,
                &[
                    ("errno", super::cp_errno_number(code)),
                    ("code", cp_box_string(code)),
                    ("syscall", cp_box_string(&syscall)),
                    ("path", cp_box_string(&cmd_str)),
                    ("spawnargs", cp_box_ptr(err_args as *const u8)),
                ],
            );
            cp_set_field(cp, b"__cpError", err);
            cp_set_field(cp, b"__cpSpawnErrno", super::cp_errno_number(code));
            cp_set_field(cp, b"exitCode", super::cp_errno_number(code));
            cp_attach_failed_pipe_sockets(cp, &stdio_kinds, pipes, false);
            let emit_closure = crate::closure::js_closure_alloc(
                crate::fn_info!(super::failed_spawn::emit_error_then_close, 0; with_declared(0)),
                1,
            );
            crate::closure::js_closure_set_capture_ptr(emit_closure, 0, cp.to_bits() as i64);
            crate::builtins::js_queue_next_tick(emit_closure as i64);
        }
    }

    cp
}


/// Node's `onexit` destroys the stdin socket before `'exit'` (or a failed
/// spawn's `'error'`).
pub(super) fn cp_destroy_stdin(cp: f64) {
    let stdin = cp_get_field(cp, b"stdin");
    if super::cp_object_ptr(stdin).is_some() {
        // SAFETY: a pipe socket; `destroy` is its own method.
        unsafe {
            crate::object::js_native_call_method(
                stdin,
                b"destroy".as_ptr() as *const i8,
                7,
                std::ptr::null(),
                0,
            );
        }
    }
}

/// Node's `onexit` ends with `process.nextTick(flushStdio)`: every readable
/// socket in `stdio` is resumed, so a pipe the program never read drains its
/// bytes and its EOF and reaches `'close'`, which the ChildProcess's own
/// `'close'` waits for. The tick gives the `'exit'` listeners one last chance
/// to start reading first. (Node skips a stream used as another child's
/// stdio; perry's `stdio` option takes no streams, so none is.)
fn cp_queue_flush_stdio(cp: f64) {
    let closure =
        crate::closure::js_closure_alloc(crate::fn_info!(cp_flush_stdio, 0; with_declared(0)), 1);
    crate::closure::js_closure_set_capture_ptr(closure, 0, cp.to_bits() as i64);
    crate::builtins::js_queue_next_tick(closure as i64);
}

extern "C" fn cp_flush_stdio(closure: *const ClosureHeader, this: crate::closure::JsThis) -> f64 {
    let scope = crate::gc::RuntimeHandleScope::new();
    let cp = scope.root_nanbox_f64(cp_this(this, closure));
    let stdio = scope.root_nanbox_f64(cp_get_field(cp.get_nanbox_f64(), b"stdio"));
    let Some(array) = super::cp_array_ptr(stdio.get_nanbox_f64()) else {
        return cp_undefined();
    };
    let count = crate::array::js_array_length(array);
    for index in 0..count {
        let Some(array) = super::cp_array_ptr(stdio.get_nanbox_f64()) else {
            break;
        };
        let stream = scope.root_nanbox_f64(crate::array::js_array_get_f64(array, index));
        if super::cp_object_ptr(stream.get_nanbox_f64()).is_none()
            || crate::value::js_is_truthy(cp_get_field(stream.get_nanbox_f64(), b"readable")) == 0
        {
            continue;
        }
        // SAFETY: a stdio socket; `resume` is its own method.
        unsafe {
            crate::object::js_native_call_method(
                stream.get_nanbox_f64(),
                b"resume".as_ptr() as *const i8,
                6,
                std::ptr::null(),
                0,
            );
        }
    }
    cp_undefined()
}


// ============================================================================
// Main-thread pump
// ============================================================================

/// Drive the reactor one tick: emit pending `spawn`/`data`/`end`/`exit`/`close`
/// for all live children. Called from `js_run_stdlib_pump`.

/// `extern "C"` wrapper registered into the armed runtime-pump slots on the
/// first live child (see `stdlib_pump::register_runtime_pump`) — programs
/// that never spawn link no reactor code.
extern "C" fn cp_reactor_pump_extern() {
    cp_reactor_pump();
}

pub(crate) fn cp_reactor_pump() {
    if CP_LIVE_COUNT.load(Ordering::Relaxed) == 0 {
        return;
    }
    if CP_PUMPING.with(|p| p.replace(true)) {
        return; // already pumping (re-entrant await inside a handler)
    }
    // turnloop P2: a child's pipes are loop operations now, so their bytes
    // exist only once the loop has been turned. A caller that drives this pump
    // without parking — the `await` poll loop, and the lifecycle tests below —
    // would otherwise spin against a queue nothing can fill. (wasm32 has no
    // turnloop, so nothing is adopted and there is nothing to drain.)
    crate::turnloop_proc::drain_pending();
    cp_reactor_pump_inner();
    CP_PUMPING.with(|p| p.set(false));
}

fn cp_reactor_pump_inner() {
    // --- Phase 0: emit `spawn` for newly-registered children. ---
    // Snapshot under a brief lock, then emit OUTSIDE the lock (emit calls user
    // callbacks which allocate / can trigger GC; the GC root scanner also locks
    // CP_LIVE, so holding it across an allocation would deadlock the same
    // thread).
    // exec/execFile children (#4912) have no JS ChildProcess, so they get no
    // `spawn` event — just mark them spawned so Phase B can close them.
    let to_spawn: Vec<(u64, u64)> = {
        let guard = cp_live_lock();
        match guard.as_ref() {
            Some(map) => map
                .iter()
                .filter(|(_, lc)| crate::agent::owns(lc.owner) && !lc.spawned)
                .map(|(h, lc)| (*h, lc.cp_bits))
                .collect(),
            None => Vec::new(),
        }
    };
    for &(handle, cp_bits) in &to_spawn {
        cp_emit(f64::from_bits(cp_bits), "spawn", &[]);
        if let Some(map) = cp_live_lock().as_mut() {
            if let Some(lc) = map.get_mut(&handle) {
                lc.spawned = true;
            }
        }
    }
    // #9535: `spawn` is a complete host-callback turn. Give an await loop or
    // the outer promise-job runner control before delivering lifecycle events
    // that a short-lived child may already have queued.
    if !to_spawn.is_empty() {
        return;
    }

    // --- Phase A: drain queued data/eof/exited events. ---
    let events = cp_take_own_events();
    for ev in events {
        match ev {
            CpEvent::Exited {
                handle,
                code,
                signal,
            } => {
                if let Some(map) = cp_live_lock().as_mut() {
                    if let Some(lc) = map.get_mut(&handle) {
                        // Windows: a kill()/timeout/abort termination surfaces
                        // here as the synthetic `TerminateProcess` exit code
                        // with no signal. Substitute the signal recorded at
                        // kill time so the exit event carries Node's
                        // `(code: null, signal: <requested>)` shape — see
                        // `LiveChild::win_kill_signal`.
                        lc.exited = Some((code, signal));
                    }
                }
            }
            CpEvent::Message { handle, json } => {
                // #1933: parse the IPC line on the main thread and deliver it as
                // a `'message'` event on the ChildProcess.
                if let Some(cp_bits) = cp_lookup_cp_bits(handle) {
                    let cp = f64::from_bits(cp_bits);
                    let sh = crate::string::js_string_from_bytes(json.as_ptr(), json.len() as u32);
                    let msg = f64::from_bits(unsafe { crate::json::js_json_parse(sh) }.bits());
                    cp_emit(cp, "message", &[msg]);
                }
            }
            CpEvent::MessageAdvanced { handle, bytes } => {
                // #2130: deserialize the V8 structured-clone payload on the main
                // thread and deliver it as a `'message'` event.
                if let Some(cp_bits) = cp_lookup_cp_bits(handle) {
                    let cp = f64::from_bits(cp_bits);
                    let msg = super::v8_serde::v8_deserialize(&bytes);
                    cp_emit(cp, "message", &[msg]);
                }
            }
            CpEvent::IpcClosed { handle } => {
                // #1933: child disconnected / exited — flip connected/channel
                // and emit `disconnect` once. The channel's close is one of
                // the ChildProcess's closes.
                cp_count_close(handle);
                if let Some(cp_bits) = cp_lookup_cp_bits(handle) {
                    let cp = f64::from_bits(cp_bits);
                    if cp_get_field(cp, b"connected").to_bits() == TAG_TRUE_F64.to_bits() {
                        cp_set_field(cp, b"connected", TAG_FALSE_F64);
                        cp_set_field(cp, b"channel", TAG_NULL_F64);
                        if let Some(map) = cp_live_lock().as_mut() {
                            if let Some(lc) = map.get_mut(&handle) {
                                lc.ipc_send = None;
                            }
                        }
                        cp_emit(cp, "disconnect", &[]);
                    }
                }
            }
            CpEvent::Timeout { handle, signal } => {
                // Mark an exec child as timed-out so its callback carries the
                // timeout error shape (#4912); a `spawn` child flips `killed`.
                let mut is_exec = false;
                {
                    let mut guard = cp_live_lock();
                    if let Some(lc) = guard.as_mut().and_then(|m| m.get_mut(&handle)) {
                        if let Some(exec) = lc.exec.as_mut() {
                            exec.timed_out = true;
                            is_exec = true;
                        }
                    }
                }
                if let Some(cp_bits) = cp_live_kill_signum(handle, signal) {
                    if !is_exec {
                        cp_set_field(f64::from_bits(cp_bits), b"killed", TAG_TRUE_F64);
                    }
                }
            }
            CpEvent::Abort { handle } => {
                let Some((cp_bits, kill_signal, signal_bits, listener_bits)) =
                    cp_take_abort_fields(handle)
                else {
                    continue;
                };
                cp_cleanup_abort_listener(signal_bits, listener_bits);
                if cp_live_kill_signal(handle, kill_signal) {
                    let cp = f64::from_bits(cp_bits);
                    cp_set_field(cp, b"killed", TAG_TRUE_F64);
                    cp_emit(cp, "error", &[cp_abort_error(None)]);
                }
            }
        }
    }

    // --- Phase B: emit `exit` once a child has exited, and `close` once every
    // close it waits for (exit, readable pipes, IPC) has happened. `close` is
    // never in exit's pump: it is deliberately left for a later pump: Node delivers it in the close-callback phase,
    // after check-phase `setImmediate` callbacks. For exec/execFile (#4912),
    // the buffered callback accompanies `close` on that following pump. ---
    let to_finish: Vec<CpCloseItem> = {
        let mut guard = cp_live_lock();
        let mut out = Vec::new();
        if let Some(map) = guard.as_mut() {
            for (h, lc) in map.iter_mut() {
                if !crate::agent::owns(lc.owner) || lc.closed || !lc.spawned {
                    continue;
                }
                if let Some((code, signal)) = lc.exited {
                    if !lc.exit_emitted || lc.closes_got >= lc.closes_needed {
                        let close = lc.exit_emitted;
                        lc.exit_emitted = true;
                        lc.closed = close;
                        out.push(CpCloseItem {
                            handle: *h,
                            code,
                            signal,
                            pid: lc.pid,
                            abort_signal_bits: if close { lc.abort_signal_bits } else { 0 },
                            abort_listener_bits: if close { lc.abort_listener_bits } else { 0 },
                            exec: if close { lc.exec.take() } else { None },
                            process_ids: lc.process_ids,
                            pipe_ids: lc.pipe_ids,
                            refed: lc.refed,
                            close,
                        });
                        if close {
                            lc.abort_signal_bits = 0;
                            lc.abort_listener_bits = 0;
                        }
                    }
                }
            }
        }
        out
    };
    for item in to_finish {
        if !item.close {
            // `close` (or the exec callback) is this child's next pump, and no
            // producer will wake the loop for it: every pipe is at EOF and the
            // waiter is done. Ask this agent's loop for that turn, or it parks
            // until some unrelated wake (its idle cap: a 1 s stall per child).
            crate::event_pump::js_notify_main_thread();
            let Some(cp_bits) = cp_lookup_cp_bits(item.handle) else {
                continue;
            };
            let cp = f64::from_bits(cp_bits);
            let code_f = item.code.map(|c| c as f64).unwrap_or(TAG_NULL_F64);
            let signal_f = item
                .signal
                .map(|s| cp_box_string(cp_signal_name(s)))
                .unwrap_or(TAG_NULL_F64);
            // Node populates exitCode/signalCode before emitting `exit`, and
            // destroys stdin first (`onexit`: `this.stdin.destroy()`).
            cp_set_field(cp, b"exitCode", code_f);
            cp_set_field(cp, b"signalCode", signal_f);
            cp_destroy_stdin(cp);
            cp_emit(cp, "exit", &[code_f, signal_f]);
            cp_queue_flush_stdio(cp);
            cp_count_close(item.handle);
            continue;
        }

        cp_finish_close(item);
    }
}

/// The ChildProcess's terminal step: the exec/execFile callback, `'close'`,
/// then the registry entry and its async resources go. Phase B runs it a pump
/// after `exit`; a pipe socket's `'close'` that is the last close runs it
/// right away, as Node's `maybeClose` does inside that socket's `'close'`.
fn cp_finish_close(item: CpCloseItem) {
    cp_cleanup_abort_listener(item.abort_signal_bits, item.abort_listener_bits);
    if let Some(exec) = item.exec {
        // exec's callback is node's reaction to the last stdio pipe closing
        // (node 26.12.1 reports PIPEWRAP), so it runs in the stdout pipe's
        // resource; a child without one falls back to the process resource.
        let ids = if item.pipe_ids[1].async_id != 0 {
            item.pipe_ids[1]
        } else {
            item.process_ids
        };
        crate::async_hook_slots::enter_resource_scope(ids);
        cp_exec_fire_close(exec, item.code, item.signal, item.pid);
        crate::async_hook_slots::leave_resource_scope(ids.async_id);
    }
    if let Some(cp_bits) = cp_lookup_cp_bits(item.handle) {
        let cp = f64::from_bits(cp_bits);
        let code_f = cp_get_field(cp, b"exitCode");
        let signal_f = cp_get_field(cp, b"signalCode");
        cp_emit(cp, "close", &[code_f, signal_f]);
    }
    // Release any loop entry still carrying one of this child's pipes
    // BEFORE the registry entry goes, because that is where the ids live.
    // #11471: the entry may already be gone — a thread exit released it
    // (`release_cp_children_in_freed_ranges`), and that release owns the
    // counter decrement. Decrement only for the removal made here.
    let removed = cp_live_lock()
        .as_mut()
        .and_then(|map| map.remove(&item.handle))
        .is_some();
    crate::async_hook_slots::destroy(item.process_ids.async_id);
    for pipe in item.pipe_ids {
        crate::async_hook_slots::destroy(pipe.async_id);
    }
    if removed {
        CP_LIVE_COUNT.fetch_sub(1, Ordering::SeqCst);
        if item.refed {
            CP_REFED_COUNT.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// A child ready to enter its terminal step in Phase B — either `exit`/`close`
/// events (`exec` is `None`) or an exec/execFile callback (`exec` is `Some`).
struct CpCloseItem {
    handle: u64,
    code: Option<i32>,
    signal: Option<i32>,
    pid: i32,
    abort_signal_bits: u64,
    abort_listener_bits: u64,
    exec: Option<Box<CpExecPending>>,
    process_ids: crate::async_hook_slots::AsyncResourceIds,
    pipe_ids: [crate::async_hook_slots::AsyncResourceIds; 3],
    refed: bool,
    close: bool,
}

/// #11471: drop every live child registered by a thread whose arena is being
/// freed.
///
/// A [`LiveChild`] holds the spawning thread's ChildProcess and stdio stream
/// objects, its AbortSignal/listener, an exec callback and stdin write
/// callbacks — all GC-rooted by `cp_reactor_scan_roots_mut`, and all emitted
/// on by the process-global pump. Left behind by an exited thread, every later
/// collection would mark/rewrite memory another arena may have reused, the
/// pump would emit `'exit'`/`'data'` onto it, and [`CP_REFED_COUNT`] would keep
/// the event loop alive forever. Values never cross threads, so an entry that
/// names the dead heap anywhere is wholly the dead thread's.
///
/// Runs in the exiting thread's TLS destructor (see `arena::thread_exit`): no
/// thread-locals, no JS, no async_hooks `destroy`. The child process itself is
/// NOT killed (Node does not kill a worker's children either); its reader and
/// waiter threads keep running and their events for this handle are skipped by
/// the pump, which already tolerates a missing entry. Dropping the entry
/// closes the retained stdin writer and IPC clone. A turnloop entry carrying a
/// pipe (`loop_streams`) lives on the dead thread's loop and goes down with it
/// rather than through `cp_release_loop_streams`, which needs that thread's
/// loop state. The counters are decremented here, for exactly the entries
/// removed here (Phase B only decrements for a removal it makes itself).
pub(crate) fn release_cp_children_in_freed_ranges(freed: &crate::arena::thread_exit::FreedRanges) {
    if CP_LIVE_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    let removed: Vec<LiveChild> = {
        let mut guard = cp_live_lock();
        let Some(map) = guard.as_mut() else {
            return;
        };
        let dead: Vec<u64> = map
            .iter()
            .filter(|(_, lc)| {
                freed.holds_bits(lc.cp_bits)
                    || lc.pipe_bits.iter().any(|&bits| freed.holds_bits(bits))
                    || freed.holds_bits(lc.abort_signal_bits)
                    || freed.holds_bits(lc.abort_listener_bits)
                    || lc
                        .exec
                        .as_ref()
                        .is_some_and(|exec| freed.holds_bits(exec.cb_bits))
                    || cp_stdin_holds_freed(lc, freed)
            })
            .map(|(handle, _)| *handle)
            .collect();
        dead.iter()
            .filter_map(|handle| map.remove(handle))
            .collect()
    };
    for lc in &removed {
        CP_LIVE_COUNT.fetch_sub(1, Ordering::SeqCst);
        if lc.refed {
            CP_REFED_COUNT.fetch_sub(1, Ordering::SeqCst);
        }
    }
    // Dropped after the lock is released: closing the stdin writer and IPC
    // clone are syscalls, and nothing here needs the registry any more.
    drop(removed);
}

/// Test probe (#11471): register a live-child entry for `cp` exactly as a
/// spawn would, minus the OS process: no pipes, no reader/waiter threads, and
/// already `spawned` and un-`ref`'d so no pump acts on it and no event loop
/// is held open by it. Returns the registry handle.
#[doc(hidden)]
pub fn cp_register_idle_live_child_for_test(cp: f64) -> u64 {
    let handle = CP_NEXT_LIVE_ID.fetch_add(1, Ordering::SeqCst);
    let no_ids = crate::async_hook_slots::AsyncResourceIds {
        async_id: 0,
        trigger_async_id: 0,
    };
    register_thread_exit_release();
    cp_live_lock().get_or_insert_with(HashMap::new).insert(
        handle,
        LiveChild {
            owner: crate::agent::current_agent(),
            cp_bits: cp.to_bits(),
            process_ids: no_ids,
            pipe_ids: [no_ids; 3],
            pipe_bits: [0; 3],
            pid: 0,
            spawned: true,
            exited: None,
            exit_emitted: false,
            closes_needed: 1,
            closes_got: 0,
            closed: false,
            refed: false,
            ipc_send: None,
            ipc_advanced: false,
            abort_signal_bits: 0,
            abort_listener_bits: 0,
            abort_kill_signal: libc_sigterm(),
            exec: None,
        },
    );
    CP_LIVE_COUNT.fetch_add(1, Ordering::SeqCst);
    handle
}

/// Test probe (#11471): is child `handle` still registered?
#[doc(hidden)]
pub fn cp_live_child_registered_for_test(handle: u64) -> bool {
    cp_live_lock()
        .as_ref()
        .is_some_and(|map| map.contains_key(&handle))
}

/// Test probes (#11471) for the reactor group's tables whose own modules are
/// `pub(crate)` (`pty::reactor`, `os::signal`), re-exported so the
/// thread-exit regression tests in `perry-stdlib` can reach them.
#[doc(hidden)]
pub mod thread_exit_test_probes {
    pub use crate::os::signal::process_signal_subscription_for_test;
    pub use crate::os::signal::set_process_signal_listener_count_for_test;
    pub use crate::pty::reactor::pty_live_registered_for_test;
    pub use crate::pty::reactor::pty_register_idle_live_for_test;
}

fn cp_stdin_holds_freed(_: &LiveChild, _: &crate::arena::thread_exit::FreedRanges) -> bool {
    false
}

fn cp_lookup_cp_bits(handle: u64) -> Option<u64> {
    cp_live_lock()
        .as_ref()
        .and_then(|map| map.get(&handle).map(|lc| lc.cp_bits))
}

pub(super) fn cp_async_scope_for_target(
    handle: u64,
    target: f64,
) -> Option<crate::async_hook_slots::AsyncResourceIds> {
    let target_bits = target.to_bits();
    let guard = cp_live_lock();
    let child = guard.as_ref()?.get(&handle)?;
    if target_bits == child.cp_bits {
        return Some(child.process_ids);
    }
    child
        .pipe_bits
        .iter()
        .enumerate()
        .filter(|(index, bits)| *index != 0 || **bits != TAG_NULL_F64.to_bits())
        .find(|(_, bits)| **bits == target_bits)
        .map(|(index, _)| child.pipe_ids[index])
}

// ============================================================================
// Live `stdin.write()` / `kill()` — called from the mod.rs method bodies.
// ============================================================================





mod integration;
mod kill;
// The lifecycle tests drive the reader/stdin-writer path; on unix the pipe
// slots are net sockets, covered by the child stdio gap tests.
mod timeout;
pub(crate) use integration::*;
pub(crate) use kill::*;
use timeout::*;
