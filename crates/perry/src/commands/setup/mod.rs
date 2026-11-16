//! `perry setup` — guided credential setup wizard for App Store / Google Play distribution
//! (and toolchain setup for the "lightweight" Windows target — LLVM + xwin'd SDK).
//!
//! Split from a single 3,145-line file into per-platform sub-modules in
//! v0.5.1020. mod.rs is a re-export hub; the public API (`SetupArgs`,
//! `run`) stays here so `commands/mod.rs` keeps importing as before.

use clap::Args;

mod macos;

// Per-platform wizards — used by `run` below.

// Cross-platform helpers shared across wizards.

// Cert utilities used by macos (re-exported in case any other module wants them).

// Per-platform per-bundle-id savers.


#[derive(Args, Debug)]
pub struct SetupArgs {
    /// Platform to configure: android, ios, visionos, macos, tvos, watchos, windows, harmonyos
    pub platform: Option<String>,

    /// (windows only) Accept the Microsoft Visual Studio Build Tools redistributable
    /// license required to download CRT + Windows SDK via xwin. Equivalent to
    /// answering "yes" at the interactive prompt; enables non-interactive / CI use.
    #[arg(long)]
    pub accept_license: bool,

    /// (ios only) Provision the currently-connected device for on-device
    /// *development* instead of App Store distribution: register the device's
    /// UDID and mint an "iOS App Development" provisioning profile via the App
    /// Store Connect API (saved to ~/.perry/<bundle>_dev.mobileprovision).
    /// Requires a prior `perry setup ios` to store API credentials.
    #[arg(long)]
    pub development: bool,
}


