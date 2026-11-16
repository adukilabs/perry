//! The zlib C ABI the dictionary-bearing contexts drive.
//!
//! Native builds use zlib-ng (`libz-ng-sys`, a C library). wasm32 has no C
//! toolchain contract for it, so it uses `libz-rs-sys`, a Rust implementation
//! of the same ABI, behind the same names: `inflate_context.rs` and
//! `deflate_context.rs` import this module as `z` and run unchanged, including
//! their custom `zalloc`/`zfree` that charge the payload ledger. Exactly one
//! zlib is linked per target.

pub use libz_ng_sys::*;


