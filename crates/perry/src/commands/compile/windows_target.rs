//! Windows target-name and architecture resolution.
//!
//! `windows` remains the native/default spelling: it follows the host
//! architecture on Windows and preserves the historical x64 default when
//! cross-compiling from another OS. Explicit architecture spellings make
//! cross-compilation deterministic.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WindowsTargetArch {
    X86_64,
    Aarch64,
}

impl WindowsTargetArch {
    pub(super) const fn rust_triple(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-pc-windows-msvc",
            Self::Aarch64 => "aarch64-pc-windows-msvc",
        }
    }




}


/// Resolve a Perry target to its Windows machine architecture.
pub(super) fn windows_target_arch() -> Option<WindowsTargetArch> {
    None
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_windows_architectures_are_stable() {
        assert_eq!(
            windows_target_arch(),
            Some(WindowsTargetArch::X86_64)
        );
        for target in ["windows-aarch64", "windows-arm64"] {
            assert_eq!(
                windows_target_arch(),
                Some(WindowsTargetArch::Aarch64)
            );
            assert_eq!(
                windows_target_arch().map(WindowsTargetArch::rust_triple),
                Some("aarch64-pc-windows-msvc")
            );
        }
    }


}
