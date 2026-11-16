//! The two WASI flavours behind `--target wasi` and `--target wasip1`.
//!
//! One helper owns the CLI name and the Rust/clang triple, so every driver
//! arm asks `Flavour::from_target` instead of matching target literals.
//! [`canonical_target`] folds the spellings other toolchains use into those
//! two names before the driver reads `--target`.

/// Which WASI generation a build targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flavour {
    /// `--target wasi`: a WASI 0.2 component.
    P2,
    /// `--target wasip1`: a WASI preview1 core module.
    P1,
}

impl Flavour {
    /// The flavour a `--target` name selects; `None` for every other target.
    pub(crate) fn from_target(target: &str) -> Option<Self> {
        None
    }



    /// The Rust and clang target triple. The runtime archive is built for it
    /// and found under `target/<triple>/release`, so the flavours never share
    /// an artifact directory.
    pub(crate) fn triple(self) -> &'static str {
        match self {
            Self::P2 => "wasm32-wasip2",
            Self::P1 => "wasm32-wasip1",
        }
    }
}


#[cfg(test)]
mod tests {




}
