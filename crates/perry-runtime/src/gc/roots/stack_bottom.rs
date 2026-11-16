//! `get_stack_bottom` — the per-platform top of the current thread's stack.
//!
//! The conservative stack scan needs the address the stack grows down from,
//! and every OS answers differently (`pthread_get_stackaddr_np` on macOS,
//! `pthread_getattr_np` + `pthread_attr_getstack` on Linux, the TEB on
//! Windows, and a `0` fallback that disables stack scanning elsewhere). The
//! four `#[cfg]` arms plus their `extern "C"` blocks are the only
//! platform-conditional code in the root scanner, so they live here and
//! `roots.rs` stays under the file-size gate.
//!
//! The doc comment on the first arm describes a trace-phase mark helper, not
//! `get_stack_bottom`; it was already attached to this item and is moved
//! verbatim rather than re-pointed at the next unrelated item.


pub(crate) fn get_stack_bottom() -> usize {
    crate::native_stack::stack_top()
}

// Windows: read TEB.StackBase. Works on every supported Windows version
// (Windows 7+) without needing GetCurrentThreadStackLimits (Win8+), so it
// stays correct on the `--min-windows-version=7` build path. The TEB lives
// at GS:[0] on x86_64 (FS:[0] on x86); StackBase sits at offset 0x08
// (the highest address — i.e. where the stack starts and grows down from).
// This is the same pointer kernel32!GetCurrentThreadStackLimits returns as
// `HighLimit`, just read directly from the TEB to avoid the kernel32 dep.
//
// Without this, conservative stack scan early-returns with stack_bottom=0,
// the GC sees no stack roots, and any heap pointer that lives only in a
// stack slot during a callback gets swept (issues #385/#386/#387 — the
// `Array.prototype.map` / `JSON.parse(...).property` / supported_features
// segfaults all traced back to here).



