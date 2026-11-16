//! Raw-mode toggle backends, split out of `readline/mod.rs` (#10750).

// ---------------------------------------------------------------------------
// Raw-mode toggle (Unix termios; Windows / non-Unix is currently a no-op
// since iOS/Android stdlib stubs handle those targets and Windows raw mode
// needs the windows-rs `Console` API which isn't a stdlib dep yet).
// ---------------------------------------------------------------------------

#[path = "termios_unix.rs"]
pub(super) mod termios_impl;


