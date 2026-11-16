use crate::fs::validate::{describe_received, is_numeric, throw_type_error_with_code};
use crate::string::{js_string_from_bytes, StringHeader};
use crate::value::{JSValue, TAG_TRUE};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

per_test_global! {
    static SIGNAL_ASYNC_IDS: LazyLock<Mutex<HashMap<String, u64>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
}

fn signal_number_by_name(name: &str) -> Option<i32> {
    {
        match name {
            "SIGHUP" => Some(libc::SIGHUP),
            "SIGINT" => Some(libc::SIGINT),
            "SIGQUIT" => Some(libc::SIGQUIT),
            "SIGILL" => Some(libc::SIGILL),
            "SIGTRAP" => Some(libc::SIGTRAP),
            "SIGABRT" => Some(libc::SIGABRT),
            "SIGIOT" => Some(libc::SIGABRT),
            "SIGBUS" => Some(libc::SIGBUS),
            "SIGFPE" => Some(libc::SIGFPE),
            "SIGKILL" => Some(libc::SIGKILL),
            "SIGUSR1" => Some(libc::SIGUSR1),
            "SIGSEGV" => Some(libc::SIGSEGV),
            "SIGUSR2" => Some(libc::SIGUSR2),
            "SIGPIPE" => Some(libc::SIGPIPE),
            "SIGALRM" => Some(libc::SIGALRM),
            "SIGTERM" => Some(libc::SIGTERM),
            "SIGCHLD" => Some(libc::SIGCHLD),
            "SIGSTKFLT" => Some(libc::SIGSTKFLT),
            "SIGCONT" => Some(libc::SIGCONT),
            "SIGSTOP" => Some(libc::SIGSTOP),
            "SIGTSTP" => Some(libc::SIGTSTP),
            "SIGTTIN" => Some(libc::SIGTTIN),
            "SIGTTOU" => Some(libc::SIGTTOU),
            "SIGURG" => Some(libc::SIGURG),
            "SIGXCPU" => Some(libc::SIGXCPU),
            "SIGXFSZ" => Some(libc::SIGXFSZ),
            "SIGVTALRM" => Some(libc::SIGVTALRM),
            "SIGPROF" => Some(libc::SIGPROF),
            "SIGWINCH" => Some(libc::SIGWINCH),
            "SIGIO" => Some(libc::SIGIO),
            "SIGPOLL" => Some(libc::SIGPOLL),
            "SIGPWR" => Some(libc::SIGPWR),
            "SIGSYS" => Some(libc::SIGSYS),
            _ => None,
        }
    }
}

static SIGNAL_WAKE_WRITE_FD: AtomicI32 = AtomicI32::new(-1);
static SIGNAL_WAKE_THREAD_STARTED: AtomicBool = AtomicBool::new(false);

static SIGHUP_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGHUP_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGHUP_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGINT_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGINT_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGINT_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGQUIT_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGQUIT_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGQUIT_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGUSR1_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGUSR1_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGUSR1_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGUSR2_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGUSR2_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGUSR2_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGABRT_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGABRT_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGABRT_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGBUS_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGBUS_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGBUS_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGPIPE_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGPIPE_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGPIPE_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGTERM_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGTERM_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGTERM_INSTALLED: AtomicBool = AtomicBool::new(false);
static SIGWINCH_PENDING: AtomicUsize = AtomicUsize::new(0);
static SIGWINCH_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SIGWINCH_INSTALLED: AtomicBool = AtomicBool::new(false);

struct ProcessSignalSlot {
    name: &'static str,
    number: libc::c_int,
    pending: &'static AtomicUsize,
    listeners: &'static AtomicUsize,
    installed: &'static AtomicBool,
}

static PROCESS_SIGNAL_SLOTS: &[ProcessSignalSlot] = &[
    ProcessSignalSlot {
        name: "SIGHUP",
        number: libc::SIGHUP,
        pending: &SIGHUP_PENDING,
        listeners: &SIGHUP_LISTENERS,
        installed: &SIGHUP_INSTALLED,
    },
    ProcessSignalSlot {
        name: "SIGINT",
        number: libc::SIGINT,
        pending: &SIGINT_PENDING,
        listeners: &SIGINT_LISTENERS,
        installed: &SIGINT_INSTALLED,
    },
    ProcessSignalSlot {
        name: "SIGQUIT",
        number: libc::SIGQUIT,
        pending: &SIGQUIT_PENDING,
        listeners: &SIGQUIT_LISTENERS,
        installed: &SIGQUIT_INSTALLED,
    },
    ProcessSignalSlot {
        name: "SIGUSR1",
        number: libc::SIGUSR1,
        pending: &SIGUSR1_PENDING,
        listeners: &SIGUSR1_LISTENERS,
        installed: &SIGUSR1_INSTALLED,
    },
    ProcessSignalSlot {
        name: "SIGUSR2",
        number: libc::SIGUSR2,
        pending: &SIGUSR2_PENDING,
        listeners: &SIGUSR2_LISTENERS,
        installed: &SIGUSR2_INSTALLED,
    },
    ProcessSignalSlot {
        name: "SIGABRT",
        number: libc::SIGABRT,
        pending: &SIGABRT_PENDING,
        listeners: &SIGABRT_LISTENERS,
        installed: &SIGABRT_INSTALLED,
    },
    ProcessSignalSlot {
        name: "SIGBUS",
        number: libc::SIGBUS,
        pending: &SIGBUS_PENDING,
        listeners: &SIGBUS_LISTENERS,
        installed: &SIGBUS_INSTALLED,
    },
    ProcessSignalSlot {
        name: "SIGPIPE",
        number: libc::SIGPIPE,
        pending: &SIGPIPE_PENDING,
        listeners: &SIGPIPE_LISTENERS,
        installed: &SIGPIPE_INSTALLED,
    },
    ProcessSignalSlot {
        name: "SIGTERM",
        number: libc::SIGTERM,
        pending: &SIGTERM_PENDING,
        listeners: &SIGTERM_LISTENERS,
        installed: &SIGTERM_INSTALLED,
    },
    // A TUI's resize listener (`process.on("SIGWINCH", …)`, opentui's).
    ProcessSignalSlot {
        name: "SIGWINCH",
        number: libc::SIGWINCH,
        pending: &SIGWINCH_PENDING,
        listeners: &SIGWINCH_LISTENERS,
        installed: &SIGWINCH_INSTALLED,
    },
];

fn slot_by_name(name: &str) -> Option<&'static ProcessSignalSlot> {
    PROCESS_SIGNAL_SLOTS.iter().find(|slot| slot.name == name)
}

fn slot_by_number(number: libc::c_int) -> Option<&'static ProcessSignalSlot> {
    PROCESS_SIGNAL_SLOTS
        .iter()
        .find(|slot| slot.number == number)
}

/// The one `sigaction` handler of every process-signal slot. SIGWINCH is also
/// `tty`'s window-change source (`stdout.on("resize")`), which installs this
/// same handler, so whichever of the two installs last serves both.
pub(crate) extern "C" fn process_signal_handler(sig: libc::c_int) {
    if sig == libc::SIGWINCH {
        crate::tty::note_window_change();
    }
    if let Some(slot) = slot_by_number(sig) {
        slot.pending.fetch_add(1, Ordering::Release);
        let fd = SIGNAL_WAKE_WRITE_FD.load(Ordering::Relaxed);
        if fd >= 0 {
            let byte = [sig as u8];
            unsafe {
                let _ = libc::write(fd, byte.as_ptr() as *const _, 1);
            }
        }
    }
}

fn set_fd_cloexec(fd: libc::c_int) {
    unsafe {
        let current = libc::fcntl(fd, libc::F_GETFD);
        if current >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFD, current | libc::FD_CLOEXEC);
        }
    }
}

fn set_fd_nonblocking(fd: libc::c_int) {
    unsafe {
        let current = libc::fcntl(fd, libc::F_GETFL);
        if current >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFL, current | libc::O_NONBLOCK);
        }
    }
}

fn ensure_signal_wake_thread() {
    if SIGNAL_WAKE_THREAD_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    unsafe {
        let mut fds = [0; 2];
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            SIGNAL_WAKE_THREAD_STARTED.store(false, Ordering::Release);
            return;
        }
        set_fd_cloexec(fds[0]);
        set_fd_cloexec(fds[1]);
        set_fd_nonblocking(fds[1]);
        SIGNAL_WAKE_WRITE_FD.store(fds[1], Ordering::Release);
        let read_fd = fds[0];
        let _ = std::thread::Builder::new()
            .name("perry-signal-wake".to_string())
            .spawn(move || {
                let mut buf = [0u8; 64];
                loop {
                    let n = libc::read(read_fd, buf.as_mut_ptr() as *mut _, buf.len());
                    if n > 0 {
                        crate::event_pump::js_notify_main_thread();
                    } else if n == 0 {
                        break;
                    } else {
                        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        if errno != libc::EINTR {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                }
            });
    }
}

// ── turnloop P2: signals as loop subscriptions ──────────────────────────────
//
// The self-pipe below is a wake primitive, and P2 exists to delete wake
// primitives. Where turnloop can carry a signal, the subscription replaces
// both the `sigaction` and the `perry-signal-wake` thread: turnloop's own
// process-wide dispatcher fans the signal out to this agent's loop, the
// completion lands on the thread that owns the JS heap, and it bumps the very
// same `pending` counter the handler used to bump — so `js_process_signal_drain`
// and every listener-count rule above are untouched.
//
// **Not every signal can move.** turnloop's portable `Signal` covers
// Int/Term/Hup/Usr1/Usr2 (plus Kill, Chld, WinCh and Break, which no slot
// here uses); Perry additionally offers SIGQUIT, SIGABRT, SIGBUS and SIGPIPE,
// which have no portable name and must not be silently dropped — SIGABRT and
// SIGBUS in particular are co-owned by the GC quarantine reporter. Those keep
// the `sigaction` path, and the wake thread now starts **only** if one of them
// is actually subscribed. A program that handles SIGINT and SIGTERM — which is
// every CLI with a graceful shutdown — starts no thread at all.

/// The portable turnloop name for a signal, or `None` when it has none and the
/// `sigaction` path must be kept.
fn turnloop_signal_for(number: libc::c_int) -> Option<turnloop::Signal> {
    match number {
        libc::SIGHUP => Some(turnloop::Signal::Hup),
        libc::SIGINT => Some(turnloop::Signal::Int),
        libc::SIGTERM => Some(turnloop::Signal::Term),
        libc::SIGUSR1 => Some(turnloop::Signal::Usr1),
        libc::SIGUSR2 => Some(turnloop::Signal::Usr2),
        _ => None,
    }
}

/// Which slots are currently subscribed through turnloop rather than through
/// `sigaction`, as a bitmask over `PROCESS_SIGNAL_SLOTS` indices. One word
/// rather than nine more statics, and the uninstall path has to know which
/// transport to unwind.
static TURNLOOP_SIGNALS: AtomicUsize = AtomicUsize::new(0);

/// The live loop-entry id per slot, so uninstall names the subscription it is
/// actually unwinding. Zero means "none". Kept next to the bitmask rather than
/// derived from the signal number, because an `off()`/`on()` pair for the same
/// signal produces two entries whose lifetimes overlap.
static TURNLOOP_SIGNAL_IDS: [AtomicUsize; 10] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// #11471: the agent whose loop carries each slot's subscription, written
/// just before [`TURNLOOP_SIGNAL_IDS`]. A turnloop id is a handle into ONE
/// agent's loop and `turnloop_proc` table, not a process-wide name, so when
/// that agent retires, [`release_signal_subscriptions_of_agent`] finds its
/// slots here and unwinds them.
static TURNLOOP_SIGNAL_OWNERS: [AtomicU64; 10] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

const _: () = assert!(
    TURNLOOP_SIGNAL_OWNERS.len() == PROCESS_SIGNAL_SLOTS.len(),
    "one turnloop owner cell per process-signal slot"
);

/// #11471: unwind every signal subscription carried by a retiring agent's
/// loop. Registered with `agent::register_retire_hook` on the first turnloop
/// install.
///
/// `retire_agent` has already shut that loop down (it runs
/// `event_pump::shutdown_agent_loop` before any retire hook), which ended the
/// subscription and restored the disposition. What survives in these
/// process-global statics is the bookkeeping: the slot's `installed` flag,
/// its turnloop bit and the dead loop's entry id. Left in place, the next
/// `process.on(signal)` on any other thread would see `installed` and never
/// subscribe — the signal silently lost — and a later `off()` would hand the
/// dead loop's id to its own loop's `signal_stop`. Clearing them returns the
/// slot to "not installed", so the next listener sync installs afresh on its
/// own loop. The listener count is left alone: it mirrors whichever thread
/// synced last, and that thread's next sync rewrites it.
fn release_signal_subscriptions_of_agent(agent: crate::agent::AgentId) {
    for (idx, slot) in PROCESS_SIGNAL_SLOTS.iter().enumerate() {
        if TURNLOOP_SIGNAL_OWNERS[idx].load(Ordering::Acquire) != agent {
            continue;
        }
        let id = TURNLOOP_SIGNAL_IDS[idx].load(Ordering::Acquire);
        // Zero: already unwound by `off()`. A failed exchange: another thread
        // unwound (or replaced) it first and owns the cleanup.
        if id == 0
            || TURNLOOP_SIGNAL_IDS[idx]
                .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            continue;
        }
        mark_on_turnloop(slot, false);
        slot.pending.store(0, Ordering::Release);
        slot.installed.store(false, Ordering::Release);
        crate::gc::census::census_after_signal_disposition_reset(slot.number);
    }
}

/// Test probe (#11471): sync `count` listeners for `name` on the calling
/// thread, exactly as `process.on`/`process.off` do.
#[doc(hidden)]
pub fn set_process_signal_listener_count_for_test(name: &str, count: usize) {
    set_process_signal_listener_count(name, count);
}

/// Test probe (#11471): `(installed, on turnloop, turnloop entry id, owner
/// agent)` for signal `name`.
#[doc(hidden)]
pub fn process_signal_subscription_for_test(name: &str) -> Option<(bool, bool, usize, u64)> {
    let slot = slot_by_name(name)?;
    let idx = slot_index(slot);
    Some((
        slot.installed.load(Ordering::Acquire),
        is_on_turnloop(slot),
        TURNLOOP_SIGNAL_IDS[idx].load(Ordering::Acquire),
        TURNLOOP_SIGNAL_OWNERS[idx].load(Ordering::Acquire),
    ))
}

// A slot added above without a matching id cell would silently index out of
// bounds at runtime; say so at compile time instead.
const _: () = assert!(
    TURNLOOP_SIGNAL_IDS.len() == PROCESS_SIGNAL_SLOTS.len(),
    "one turnloop id cell per process-signal slot"
);

fn slot_index(slot: &'static ProcessSignalSlot) -> usize {
    PROCESS_SIGNAL_SLOTS
        .iter()
        .position(|candidate| candidate.number == slot.number)
        .unwrap_or(0)
}

fn mark_on_turnloop(slot: &'static ProcessSignalSlot, on: bool) {
    let bit = 1usize << slot_index(slot);
    if on {
        TURNLOOP_SIGNALS.fetch_or(bit, Ordering::AcqRel);
    } else {
        TURNLOOP_SIGNALS.fetch_and(!bit, Ordering::AcqRel);
    }
}

fn is_on_turnloop(slot: &'static ProcessSignalSlot) -> bool {
    TURNLOOP_SIGNALS.load(Ordering::Acquire) & (1usize << slot_index(slot)) != 0
}

/// Whether any signal is still being delivered through turnloop. The "subject
/// ran" predicate a test needs: a claim that signals moved off the thread is
/// only worth making if this was ever true.
pub fn signals_on_turnloop() -> usize {
    TURNLOOP_SIGNALS.load(Ordering::Acquire).count_ones() as usize
}


/// Subscribe this agent's loop to `slot`. Returns whether it took.
fn install_signal_on_loop(slot: &'static ProcessSignalSlot) -> bool {
    let Some(signal) = turnloop_signal_for(slot.number) else {
        return false;
    };
    if !crate::turnloop_proc::available() {
        return false;
    }
    let Ok(id) = crate::turnloop_proc::signal_start(
        signal,
        crate::turnloop_proc::Owner::ProcessSignal {
            signum: slot.number,
        },
    ) else {
        return false;
    };
    // #11471: the owner first, so a retire hook that sees this id also sees
    // whose loop it names.
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        crate::agent::register_retire_hook(release_signal_subscriptions_of_agent);
    });
    TURNLOOP_SIGNAL_OWNERS[slot_index(slot)]
        .store(crate::agent::current_agent(), Ordering::Release);
    TURNLOOP_SIGNAL_IDS[slot_index(slot)].store(id as usize, Ordering::Release);
    // A registered listener is ref-NEUTRAL (see `has_active_process_signal_listeners`
    // and `crates/perry/tests/issue_signal_listener_ref_neutral.rs`): it must
    // not by itself keep the process alive. Unreffing the subscription encodes
    // that in the transport instead of leaving it to a second counter to undo.
    crate::turnloop_proc::set_ref(id, false);
    mark_on_turnloop(slot, true);
    true
}


/// One signal completion, delivered on the loop-owning thread.
///
/// Does exactly what the `sigaction` handler plus the wake thread did between
/// them — bump the slot's pending count, wake the loop — with neither the
/// async-signal-safety constraint nor the thread. Everything downstream
/// (`take_pending_process_signals`, `js_process_signal_drain`, the listener
/// re-sync) is unchanged.
pub(crate) fn on_signal_completion(signum: i32, event: crate::turnloop_proc::StreamEvent) {
    use crate::turnloop_proc::StreamEvent;
    match event {
        StreamEvent::Signal => {
            if let Some(slot) = slot_by_number(signum) {
                slot.pending.fetch_add(1, Ordering::Release);
            }
            crate::event_pump::js_notify_main_thread();
        }
        // The terminal completion of a subscription that has already been
        // unwound. `uninstall_process_signal_handler` cleared the bit and the
        // id, so there is nothing left to release; it is named here only to
        // make the exhaustive match say so.
        StreamEvent::Closed => {}
        _ => {}
    }
}

fn install_process_signal_handler(slot: &'static ProcessSignalSlot) {
    if slot
        .installed
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    if install_signal_on_loop(slot) {
        return;
    }

    // No portable turnloop name, or no loop on this thread: keep the pre-P2
    // transport verbatim, self-pipe and wake thread included. The thread is
    // started HERE rather than unconditionally, so it exists only for a
    // program that actually subscribes to one of the signals turnloop cannot
    // carry.
    ensure_signal_wake_thread();
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = process_signal_handler as *const () as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        if libc::sigaction(slot.number, &sa, std::ptr::null_mut()) != 0 {
            slot.installed.store(false, Ordering::Release);
        }
    }
}

fn uninstall_process_signal_handler(slot: &'static ProcessSignalSlot) {
    slot.pending.store(0, Ordering::Release);
    if slot
        .installed
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    if is_on_turnloop(slot) {
        // turnloop restores the previous disposition when the subscription
        // ends; the bit is cleared by the terminal completion, not here, so
        // the unwind stays exactly-once (DESIGN D4).
        {
            let id = TURNLOOP_SIGNAL_IDS[slot_index(slot)].swap(0, Ordering::AcqRel);
            if id != 0 {
                crate::turnloop_proc::signal_stop(id as u64);
            }
        }
        mark_on_turnloop(slot, false);
    } else if slot.number != libc::SIGWINCH {
        // SIGWINCH keeps the shared handler: `tty`'s resize source may still
        // need it, and the default disposition ignores the signal anyway.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut sa.sa_mask);
            let _ = libc::sigaction(slot.number, &sa, std::ptr::null_mut());
        }
    }
    // `PERRY_GC_CENSUS` owns SIGUSR2 for the life of the process; give it
    // back after the JS listener's disposition reset. No-op when unset.
    crate::gc::census::census_after_signal_disposition_reset(slot.number);
}

// ============================================================================
// Windows: console control events → process signal events (#6609 audit).
//
// Mirrors the Unix structure above, with `SetConsoleCtrlHandler` standing in
// for `sigaction` and the mapping Node/libuv use on Windows:
//
//   CTRL_C_EVENT     → 'SIGINT'
//   CTRL_BREAK_EVENT → 'SIGBREAK'
//   CTRL_CLOSE_EVENT → 'SIGHUP'   (console closed; ~5s grace before force-kill)
//   'SIGTERM'        → registerable but source-less (no console event raises
//                      it — Node accepts the listener and it simply never
//                      fires from the console).
//
// The control handler runs on a Windows-spawned thread, never the JS main
// thread, so it must not touch the JS heap: it only bumps the slot's
// `pending` atomic and calls `js_notify_main_thread()` (atomics + condvar,
// documented safe from any thread). The main thread drains via the same
// platform-neutral path Unix uses: `take_pending_process_signals()` from
// `js_process_signal_drain` on the next event-loop tick.






pub(crate) fn is_process_signal_name(name: &str) -> bool {
    {
        slot_by_name(name).is_some()
    }
}

pub(crate) fn set_process_signal_listener_count(name: &str, count: usize) {
    {
        let Some(slot) = slot_by_name(name) else {
            return;
        };
        slot.listeners.store(count, Ordering::Release);
        if count > 0 {
            install_process_signal_handler(slot);
        } else {
            uninstall_process_signal_handler(slot);
        }
    }

    if count > 0 {
        let needs_resource = !SIGNAL_ASYNC_IDS.lock().unwrap().contains_key(name);
        if needs_resource {
            crate::arena::thread_exit::register_thread_exit_range_hook(
                release_signal_resources_in_freed_ranges,
            );
            let async_id = unsafe {
                crate::async_hook_slots::js_async_hooks_provider_init(b"SIGNALWRAP".as_ptr(), 10)
            };
            SIGNAL_ASYNC_IDS
                .lock()
                .unwrap()
                .entry(name.to_string())
                .or_insert(async_id);
        }
    } else {
        let async_id = SIGNAL_ASYNC_IDS.lock().unwrap().remove(name);
        if let Some(async_id) = async_id {
            crate::async_hook_slots::destroy(async_id);
        }
    }
}

/// Whether a *pending, undelivered* signal is waiting to be drained.
///
/// Node semantics: a registered `process.on('SIGINT', …)` listener is
/// ref-NEUTRAL — it does NOT keep the event loop alive on its own (the
/// docs' own example calls `process.stdin.resume()` precisely because a
/// signal listener alone won't keep the process running). The loop must
/// only be held open when a signal has actually arrived and its
/// listener callbacks still need to fire on the main thread.
///
/// Pre-fix this returned `listeners > 0`, so any CLI that installs a
/// SIGINT/SIGTERM/SIGHUP handler at startup (the common graceful-shutdown
/// pattern) pinned the event loop forever: once its real work drained,
/// the loop had no microtasks/timers/async ops left, yet
/// `js_stdlib_has_active_handles` kept returning 1 from this check and
/// the program hung at idle instead of exiting. Now we gate on a pending
/// signal so the listener registration alone no longer keeps the loop
/// alive; a delivered signal still wakes the loop via the self-pipe
/// notify and is drained by `js_process_signal_drain` on the next tick.
pub(crate) fn has_active_process_signal_listeners() -> bool {
    {
        PROCESS_SIGNAL_SLOTS.iter().any(|slot| {
            slot.pending.load(Ordering::Acquire) > 0 && slot.listeners.load(Ordering::Acquire) > 0
        })
    }
}

pub(crate) fn take_pending_process_signals() -> Vec<&'static str> {
    {
        let mut signals = Vec::new();
        for slot in PROCESS_SIGNAL_SLOTS {
            let count = slot.pending.swap(0, Ordering::AcqRel);
            if count == 0 || slot.listeners.load(Ordering::Acquire) == 0 {
                continue;
            }
            signals.extend(std::iter::repeat_n(slot.name, count));
        }
        signals
    }
}




fn signal_names() -> Vec<&'static str> {
    let mut names = vec![
        "SIGHUP", "SIGINT", "SIGQUIT", "SIGILL", "SIGTRAP", "SIGABRT", "SIGIOT", "SIGBUS",
        "SIGFPE", "SIGKILL", "SIGUSR1", "SIGSEGV", "SIGUSR2", "SIGPIPE", "SIGALRM", "SIGTERM",
        "SIGCHLD",
    ];
    names.push("SIGSTKFLT");
    names.extend([
        "SIGCONT",
        "SIGSTOP",
        "SIGTSTP",
        "SIGTTIN",
        "SIGTTOU",
        "SIGURG",
        "SIGXCPU",
        "SIGXFSZ",
        "SIGVTALRM",
        "SIGPROF",
        "SIGWINCH",
        "SIGIO",
    ]);
    names.push("SIGPOLL");
    names.push("SIGPWR");
    names.push("SIGSYS");
    names
}

fn read_js_string(value: f64) -> Option<String> {
    let jv = JSValue::from_bits(value.to_bits());
    if !jv.is_any_string() {
        return None;
    }
    let ptr = crate::value::js_get_string_pointer_unified(value) as *const StringHeader;
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let len = (*ptr).byte_len as usize;
        let data = (ptr as *const u8).add(std::mem::size_of::<StringHeader>());
        Some(String::from_utf8_lossy(std::slice::from_raw_parts(data, len)).into_owned())
    }
}

fn numeric_value(jv: JSValue) -> Option<f64> {
    if jv.is_number() {
        Some(jv.as_number())
    } else {
        None
    }
}

fn is_array_value(jv: JSValue) -> bool {
    if !jv.is_pointer() {
        return false;
    }
    let ptr = jv.as_pointer::<u8>();
    if ptr.is_null() || (ptr as usize) < crate::gc::GC_HEADER_SIZE + 0x1000 {
        return false;
    }
    let header = unsafe { &*(ptr.sub(crate::gc::GC_HEADER_SIZE) as *const crate::gc::GcHeader) };
    crate::gc_type_is_array!(header.obj_type)
}

fn display_value(value: f64) -> String {
    if let Some(s) = read_js_string(value) {
        return s;
    }
    let jv = JSValue::from_bits(value.to_bits());
    if jv.is_undefined() {
        return "undefined".to_string();
    }
    if jv.is_null() {
        return "null".to_string();
    }
    if jv.is_bool() {
        return jv.as_bool().to_string();
    }
    if let Some(n) = numeric_value(jv) {
        if n.is_nan() {
            return "NaN".to_string();
        }
        if n == f64::INFINITY {
            return "Infinity".to_string();
        }
        if n == f64::NEG_INFINITY {
            return "-Infinity".to_string();
        }
        if n.is_finite() && n.fract() == 0.0 {
            return format!("{}", n as i64);
        }
        return format!("{n}");
    }
    if is_array_value(jv) {
        return "[]".to_string();
    }
    if jv.is_pointer() {
        return "{}".to_string();
    }
    describe_received(value)
}

fn throw_unknown_signal(value: f64) -> ! {
    let message = format!("Unknown signal: {}", display_value(value));
    throw_type_error_with_code(&message, "ERR_UNKNOWN_SIGNAL")
}

fn throw_invalid_signal_code(value: f64) -> ! {
    let expected = signal_names()
        .into_iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let received = if let Some(s) = read_js_string(value) {
        format!("'{s}'")
    } else {
        display_value(value)
    };
    let message =
        format!("The argument 'signalCode' must be one of: {expected}. Received {received}");
    throw_type_error_with_code(&message, "ERR_INVALID_ARG_VALUE")
}

fn normalize_process_signal(signal: f64) -> i32 {
    let jv = JSValue::from_bits(signal.to_bits());
    if jv.is_undefined() {
        return signal_number_by_name("SIGTERM").unwrap_or(15);
    }
    if jv.is_null() {
        return 0;
    }
    if let Some(name) = read_js_string(signal) {
        return signal_number_by_name(&name).unwrap_or_else(|| throw_unknown_signal(signal));
    }
    if let Some(n) = numeric_value(jv) {
        if n.is_nan() || n == 0.0 {
            return 0;
        }
        if !n.is_finite() || n.fract() != 0.0 || n < i32::MIN as f64 || n > i32::MAX as f64 {
            throw_unknown_signal(signal);
        }
        return n as i32;
    }
    throw_unknown_signal(signal)
}

fn kill_errno_code(errno: i32) -> &'static str {
    match errno {
        x if x == libc::EINVAL => "EINVAL",
        x if x == libc::ESRCH => "ESRCH",
        x if x == libc::EPERM => "EPERM",
        x if x == libc::EINTR => "EINTR",
        _ => "EIO",
    }
}

/// Throw the Node-shaped `process.kill` failure: an `Error` whose message is
/// `kill <CODE>` with `.code = <CODE>` and `.syscall = "kill"`. Shared by the
/// Unix (errno-derived) and Windows (Win32-error-derived) arms so both report
/// failures identically.
fn throw_kill_error_code(code: &'static str) -> ! {
    let message = format!("kill {code}");
    let msg = js_string_from_bytes(message.as_ptr(), message.len() as u32);
    crate::node_submodules::register_error_code_pub(msg, code);
    crate::node_submodules::register_error_syscall(msg, "kill");
    crate::exception::throw_core(crate::error::CoreError::error(msg))
}

fn throw_kill_error(errno: i32) -> ! {
    throw_kill_error_code(kill_errno_code(errno))
}



/// process.kill(pid, signal?) — send signal to process. signal=0 means
/// existence check, and omitted/undefined signal defaults to SIGTERM.
#[no_mangle]
pub extern "C" fn js_process_kill(pid: f64, signal: f64) -> f64 {
    let pid_jv = JSValue::from_bits(pid.to_bits());
    if !is_numeric(pid_jv) {
        let message = format!(
            "The \"pid\" argument must be of type number. Received {}",
            describe_received(pid)
        );
        throw_type_error_with_code(&message, "ERR_INVALID_ARG_TYPE");
    }

    let pid_i = pid_jv.as_number() as i32;
    let sig_i = normalize_process_signal(signal);
    unsafe {
        if libc::kill(pid_i, sig_i) != 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            throw_kill_error(errno);
        }
    }
    f64::from_bits(TAG_TRUE)
}

#[no_mangle]
pub extern "C" fn js_util_convert_process_signal_to_exit_code(signal_code: f64) -> f64 {
    let Some(name) = read_js_string(signal_code) else {
        throw_invalid_signal_code(signal_code);
    };
    let Some(signal) = signal_number_by_name(&name) else {
        throw_invalid_signal_code(signal_code);
    };
    (128 + signal) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_exit_codes_match_linux_node() {
        assert_eq!(signal_number_by_name("SIGTERM"), Some(15));
        assert_eq!(signal_number_by_name("SIGINT"), Some(2));
        assert_eq!(signal_number_by_name("SIGKILL"), Some(9));
        assert_eq!(signal_number_by_name("sigterm"), None);
    }







}

/// Signal registrations are native leases. A retiring mutator frees their
/// pinned state cells, so process-wide routing must release those tokens too.
fn release_signal_resources_in_freed_ranges(freed: &crate::arena::thread_exit::FreedRanges) {
    if let Ok(mut ids) = SIGNAL_ASYNC_IDS.lock() {
        ids.retain(|_, token| !freed.contains(*token as usize));
    }
}

#[cfg(test)]
mod resource_teardown_tests {
    #[test]
    fn native_signal_tokens_clear_with_their_retiring_allocation_range() {
        check(false);
    }
    #[test]
    #[ignore = "negative control: omit the native signal allocation-range invalidation"]
    fn negative_control_objtables_signal_resource_teardown() {
        check(true);
    }
    fn check(omit: bool) {
        let _guard = crate::gc::global_side_table_test_lock();
        let token = unsafe {
            crate::async_hook_slots::js_async_hooks_provider_init(b"SIGNALWRAP".as_ptr(), 10)
        };
        assert!(crate::async_hook_slots::resource_state::get(token).is_some());
        const NAME: &str = "objtables-native-signal-range";
        super::SIGNAL_ASYNC_IDS
            .lock()
            .unwrap()
            .insert(NAME.into(), token);
        let ranges =
            crate::arena::thread_exit::FreedRanges::new(&[(token as usize, token as usize + 1)]);
        if !omit {
            super::release_signal_resources_in_freed_ranges(&ranges);
        }
        assert!(
            !super::SIGNAL_ASYNC_IDS.lock().unwrap().contains_key(NAME),
            "retired signal state must not remain in native routing"
        );
        crate::async_hook_slots::destroy(token);
    }
}
