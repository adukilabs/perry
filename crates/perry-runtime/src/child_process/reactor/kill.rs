//! Kill and signal delivery machinery, split out of `reactor.rs` to keep it under the 2000-line cap.
//! A child module, so `use super::*` reaches the reactor's private state.

use super::*;





/// What the platform kill primitive acts on: the pid for unix `libc::kill`,
/// the spawn-time duplicated process handle on Windows (immune to pid reuse —
/// see `cp_win_kill`).
#[inline]
pub(super) fn cp_kill_target(lc: &LiveChild) -> i32 {
    lc.pid
}

pub(super) fn cp_live_kill_signum(handle: u64, signum: i32) -> Option<u64> {
    let (target, cp_bits) = {
        let guard = cp_live_lock();
        match guard.as_ref().and_then(|map| map.get(&handle)) {
            // Skip if already reaped — the pid may have been recycled by the OS.
            Some(lc) if lc.exited.is_none() => (cp_kill_target(lc), lc.cp_bits),
            _ => return None,
        }
    };
    {
        if unsafe { libc::kill(target, signum) == 0 } {
            Some(cp_bits)
        } else {
            None
        }
    }
}

/// Signal a live child. `signal` is the JS `kill([signal])` argument (a signal
/// name string, a number, or — for the no-arg / default case — undefined or the
/// `0.0` arg-padding, both treated as `SIGTERM`). Returns whether the signal
/// was delivered.
pub(crate) fn cp_live_kill(handle: u64, signal: f64) -> bool {
    cp_live_kill_signum(handle, cp_signal_from_value(signal)).is_some()
}

pub(super) fn cp_live_kill_signal(handle: u64, signum: i32) -> bool {
    let target = {
        let guard = cp_live_lock();
        match guard.as_ref().and_then(|map| map.get(&handle)) {
            // Skip if already reaped — the pid may have been recycled by the OS.
            Some(lc) if lc.exited.is_none() => cp_kill_target(lc),
            _ => return false,
        }
    };
    {
        unsafe { libc::kill(target, signum) == 0 }
    }
}

/// Map a JS `kill` signal argument to a Unix signal number. Default / no-arg
/// (`undefined` or the `0.0` padding) → `SIGTERM`.
pub(super) fn cp_parse_signal(signal: f64) -> i32 {
    const SIGTERM: i32 = libc::SIGTERM;
    if JSValue::from_bits(signal.to_bits()).is_undefined() {
        return SIGTERM;
    }
    // Numeric forms BEFORE the string lookup: the unified string accessor
    // coerces numbers to "9"-style strings, which are not signal names; an
    // int32 can also arrive NaN-boxed, which `is_finite()` alone misses.
    if signal.is_finite() {
        let n = signal as i32;
        // 0 is the "no-arg" padding sentinel — treat as the default SIGTERM.
        return if n == 0 { SIGTERM } else { n };
    }
    if let Some(name) = cp_value_to_string(signal) {
        return cp_signal_number(&name).unwrap_or(SIGTERM);
    }
    SIGTERM
}

/// Inverse of `super::cp_signal_name` for the common signals.
pub(super) fn cp_signal_number(name: &str) -> Option<i32> {
    Some(match name {
        "SIGHUP" => libc::SIGHUP,
        "SIGINT" => libc::SIGINT,
        "SIGQUIT" => libc::SIGQUIT,
        "SIGABRT" => libc::SIGABRT,
        "SIGKILL" => libc::SIGKILL,
        "SIGTERM" => libc::SIGTERM,
        "SIGUSR1" => libc::SIGUSR1,
        "SIGUSR2" => libc::SIGUSR2,
        "SIGSTOP" => libc::SIGSTOP,
        "SIGCONT" => libc::SIGCONT,
        _ => return None,
    })
}
