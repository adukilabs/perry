//! Limits a host program can hold compiled code to.
//!
//! A program that links `--output-type staticlib` output and calls into it (an
//! app runner, a plugin host) needs to bound a call in time and in memory and
//! keep running afterwards. Three things make that possible, all here:
//!
//! * [`js_host_call`] runs a body under a base trap, the one a Worker thread's
//!   body runs under (`exception::run_worker_body`). A stop unwinds to it past
//!   every `catch` and `finally` of the program.
//! * [`js_host_stop`] asks the running call to stop. It only writes atomics, so
//!   a signal handler or another thread may call it. The call stops at the next
//!   point compiled code already checks: a loop back-edge
//!   (`loop_interrupt::js_loop_interrupt`) or a function's stack check
//!   (`stack_guard::js_stack_overflow`, made to fire by raising the published
//!   limit above every address). Runtime loops that can run long without
//!   entering compiled code ask [`due`] and stop through [`stop_now`].
//! * [`js_host_memory_limit`] sets an allowance. The global allocator counts
//!   live bytes ([`Metered`]), and the arena reports its blocks ([`arena`]).
//!   Over the allowance, block reservation and `gc_malloc` run the collector's
//!   emergency collection first, a byte store is refused with a `RangeError`
//!   before it is taken, and anything still over asks the call to stop.
//!
//! * [`js_host_bigint_limit`] lowers the largest BigInt, which bounds the one
//!   kind of runtime call that can be neither stopped nor refused for memory.
//!
//! A stop is a throw the program cannot catch, made where a throw can already
//! happen. It abandons the same frames a throw from that point abandons.
//!
//! One host call at a time, on one thread. Nothing here is per agent.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// [`js_host_call`] results.
pub const RETURNED: i32 = 0;
/// The body threw. The value is in the exception slot (`js_get_exception`).
pub const THREW: i32 = 1;
/// The call was stopped by [`js_host_stop`].
pub const STOPPED: i32 = 2;
/// The call was stopped because the program is over its memory allowance.
pub const OVER_MEMORY: i32 = 3;

/// The outstanding stop request: 0, [`STOPPED`] or [`OVER_MEMORY`].
static STOP: AtomicU32 = AtomicU32::new(0);
/// A host call is running.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The stack-limit slot of the thread running the host call, as an atomic so
/// [`js_host_stop`] can write it from a signal handler or another thread.
static SLOT: AtomicPtr<AtomicUsize> = AtomicPtr::new(std::ptr::null_mut());
/// The memory allowance in bytes. 0: none.
static LIMIT: AtomicUsize = AtomicUsize::new(0);
/// Live bytes handed out by the global allocator.
static HEAP: AtomicUsize = AtomicUsize::new(0);
/// Bytes of arena blocks, as last reported by the host call's thread.
static ARENA: AtomicUsize = AtomicUsize::new(0);
/// The largest BigInt in bits. The default is the runtime's own limit.
static BIGINT: AtomicU64 = AtomicU64::new(1 << 30);
/// What the program held, with what it was asking for, when it was last
/// refused. Memory is given back before the host can ask.
static REFUSED: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread is the one running the host call.
    static IN_CALL: Cell<bool> = const { Cell::new(false) };
    /// A stop was taken on this thread during the current host call.
    static TAKEN: Cell<bool> = const { Cell::new(false) };
}

fn request(reason: u32) {
    if STOP
        .compare_exchange(0, reason, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        crate::loop_interrupt::PERRY_LOOP_INTERRUPT_ARMED.fetch_add(1, Ordering::SeqCst);
        let slot = SLOT.load(Ordering::SeqCst);
        if !slot.is_null() {
            // Above every stack address: the next stack check takes its
            // overflow arm, which asks `stop_here` first.
            unsafe { (*slot).store(usize::MAX, Ordering::SeqCst) };
        }
    }
}

/// Retire any request and put the real stack limit back.
fn reset() {
    if STOP.swap(0, Ordering::SeqCst) != 0 {
        crate::loop_interrupt::PERRY_LOOP_INTERRUPT_ARMED.fetch_sub(1, Ordering::SeqCst);
    }
    restore_stack_limit();
}

fn restore_stack_limit() {
    let slot = SLOT.load(Ordering::SeqCst);
    if !slot.is_null() {
        unsafe { (*slot).store(crate::stack_guard::limit(), Ordering::SeqCst) };
    }
}

/// Whether this thread is inside a host call.
#[inline]
pub(crate) fn calling() -> bool {
    IN_CALL.with(Cell::get)
}

/// The largest BigInt in bits ([`js_host_bigint_limit`]).
#[inline]
pub(crate) fn bigint() -> u64 {
    BIGINT.load(Ordering::Relaxed)
}

/// Whether the call running on this thread has been asked to stop.
#[inline]
pub(crate) fn due() -> bool {
    STOP.load(Ordering::Relaxed) != 0 && IN_CALL.with(Cell::get)
}

/// End the host call running on this thread if it was asked to stop; otherwise
/// return. The caller must be at a point where a throw may happen.
pub(crate) fn stop_here() {
    if !due() {
        return;
    }
    restore_stack_limit();
    TAKEN.with(|t| t.set(true));
    crate::exception::hand_to_worker_base(f64::from_bits(crate::value::TAG_UNDEFINED));
}

/// [`stop_here`] for a caller that has already seen [`due`] and has dropped
/// what it owns.
pub(crate) fn stop_now() -> ! {
    stop_here();
    eprintln!("perry: FATAL: a host stop was due outside a host call");
    std::process::abort();
}

/// Run `body(ctx)` as one bounded call. Returns [`RETURNED`], [`THREW`],
/// [`STOPPED`] or [`OVER_MEMORY`]. After [`THREW`] the value is in the
/// exception slot, not cleared.
///
/// # Safety
/// `body` must be safe to call with `ctx`. Not reentrant.
#[no_mangle]
pub unsafe extern "C" fn js_host_call(
    body: crate::exception::TrapBody,
    ctx: *mut core::ffi::c_void,
) -> i32 {
    crate::stack_guard::publish_stack_limit();
    SLOT.store(
        crate::agent_ptrs::slot_addr(crate::agent_ptrs::AGENT_PTR_STACK_LIMIT).cast(),
        Ordering::SeqCst,
    );
    reset();
    TAKEN.with(|t| t.set(false));
    IN_CALL.with(|c| c.set(true));
    arena(crate::arena::arena_total_bytes());
    ACTIVE.store(true, Ordering::SeqCst);
    let returned = crate::exception::trap_base(body, ctx);
    ACTIVE.store(false, Ordering::SeqCst);
    let reason = STOP.load(Ordering::SeqCst) as i32;
    let taken = TAKEN.with(|t| t.replace(false));
    reset();
    IN_CALL.with(|c| c.set(false));
    // The stack limit is published by the thread's first allocation, which
    // can be inside the first call.
    restore_stack_limit();
    let outcome = if taken {
        crate::exception::js_clear_exception();
        reason
    } else if !returned {
        THREW
    } else {
        RETURNED
    };
    if reason == OVER_MEMORY && outcome != THREW {
        // Nothing of the program is running: collect, then say whether what
        // it keeps is still over its allowance.
        crate::gc::js_gc_collect();
        arena(crate::arena::arena_total_bytes());
        if taken || over(0) {
            return OVER_MEMORY;
        }
    }
    arena(crate::arena::arena_total_bytes());
    outcome
}

/// Ask the running host call to stop. Async-signal-safe. A host that finds the
/// call still running may ask again: the stack check's limit is rewritten by
/// the thread itself while it builds a `RangeError`, which can undo one ask.
#[no_mangle]
pub extern "C" fn js_host_stop() {
    if ACTIVE.load(Ordering::SeqCst) {
        request(STOPPED as u32);
        let slot = SLOT.load(Ordering::SeqCst);
        if !slot.is_null() && STOP.load(Ordering::SeqCst) != 0 {
            unsafe { (*slot).store(usize::MAX, Ordering::SeqCst) };
        }
    }
}

/// Bytes the program held, with what it was asking for, when it was last
/// refused memory. 0 when it never was.
#[no_mangle]
pub extern "C" fn js_host_memory_refused() -> usize {
    REFUSED.load(Ordering::Relaxed)
}

/// Set the largest BigInt in bits; a result over it is the `RangeError` the
/// runtime's own limit of 2^30 bits raises. One BigInt operation is a call
/// into a library that cannot be stopped part-way, and its time grows faster
/// than its operands: `7n ** 3000000n` ran for seconds. A limit bounds every
/// single operation, and the code between two operations is stoppable.
/// Values over 2^30 are held to 2^30.
#[no_mangle]
pub extern "C" fn js_host_bigint_limit(bits: u64) {
    BIGINT.store(bits.clamp(64, 1 << 30), Ordering::SeqCst);
}

/// Set the memory allowance in bytes; 0 removes it. Call it before the first
/// [`js_host_call`]: the collector sizes itself from it once.
#[no_mangle]
pub extern "C" fn js_host_memory_limit(bytes: usize) {
    LIMIT.store(bytes, Ordering::SeqCst);
}

/// Bytes the runtime holds: live allocator bytes plus arena blocks.
#[no_mangle]
pub extern "C" fn js_host_memory_used() -> usize {
    HEAP.load(Ordering::Relaxed) + ARENA.load(Ordering::Relaxed)
}

/// The allowance, for the collector's budget.
pub(crate) fn limit() -> usize {
    LIMIT.load(Ordering::Relaxed)
}

/// Report this thread's arena block bytes. Only the host call's thread counts.
pub(crate) fn arena(bytes: usize) {
    if IN_CALL.with(Cell::get) {
        ARENA.store(bytes, Ordering::Relaxed);
    }
}

/// Whether taking `extra` more bytes would put a running host call over its
/// allowance. False once the call has been asked to stop: it is leaving.
#[inline]
pub(crate) fn over(extra: usize) -> bool {
    let limit = LIMIT.load(Ordering::Relaxed);
    limit != 0
        && STOP.load(Ordering::Relaxed) == 0
        && HEAP.load(Ordering::Relaxed) + ARENA.load(Ordering::Relaxed) + extra > limit
}

/// Record a request for `extra` bytes that was turned down with an error the
/// program can catch.
pub(crate) fn note(extra: usize) {
    REFUSED.store(
        HEAP.load(Ordering::Relaxed) + ARENA.load(Ordering::Relaxed) + extra,
        Ordering::Relaxed,
    );
}

/// The program is over its allowance, asking for `extra` more bytes, and
/// nothing could be reclaimed.
#[cold]
pub(crate) fn refuse(extra: usize) {
    if ACTIVE.load(Ordering::SeqCst) && STOP.load(Ordering::SeqCst) == 0 {
        REFUSED.store(
            HEAP.load(Ordering::Relaxed) + ARENA.load(Ordering::Relaxed) + extra,
            Ordering::Relaxed,
        );
        request(OVER_MEMORY as u32);
    }
}

#[inline]
fn grew(bytes: usize) {
    let live = HEAP.fetch_add(bytes, Ordering::Relaxed) + bytes;
    let limit = LIMIT.load(Ordering::Relaxed);
    if limit != 0 && live + ARENA.load(Ordering::Relaxed) > limit {
        refuse(0);
    }
}

/// The `#[global_allocator]` wrapper that counts live bytes. Two relaxed
/// atomic updates per allocation and free.
pub struct Metered<A: GlobalAlloc>(pub A);

unsafe impl<A: GlobalAlloc> GlobalAlloc for Metered<A> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { self.0.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { self.0.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }
    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        HEAP.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { self.0.dealloc(ptr, layout) }
    }
    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { self.0.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                grew(new_size - layout.size());
            } else {
                HEAP.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}

// Entry points of an embedding host, referenced by nothing in the archive.
#[used(compiler)]
static KEEP_HOST_CALL: unsafe extern "C" fn(crate::exception::TrapBody, *mut core::ffi::c_void) -> i32 =
    js_host_call;
#[used(compiler)]
static KEEP_HOST_STOP: extern "C" fn() = js_host_stop;
#[used(compiler)]
static KEEP_HOST_MEMORY_LIMIT: extern "C" fn(usize) = js_host_memory_limit;
#[used(compiler)]
static KEEP_HOST_MEMORY_USED: extern "C" fn() -> usize = js_host_memory_used;
#[used(compiler)]
static KEEP_HOST_MEMORY_REFUSED: extern "C" fn() -> usize = js_host_memory_refused;
