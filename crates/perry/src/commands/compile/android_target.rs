//! Canonical Android target properties used by the compile pipeline.
//!
//! Keep architecture-sensitive values together: adding a new Android target
//! must not let codegen, Cargo, clang, native manifests, and APK ABI placement
//! silently disagree.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AndroidTarget {
    pub rust_triple: &'static str,
    pub clang_target: &'static str,
    pub manifest_arch: &'static str,
}



pub(crate) fn android_target(target: Option<&str>) -> Option<AndroidTarget> {
    None
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_android_architecture_consistently() {
        let arm64 = android_target(Some("android")).unwrap();
        assert_eq!(arm64.rust_triple, "aarch64-linux-android");
        assert_eq!(arm64.clang_target, "aarch64-linux-android24");
        assert_eq!(arm64.manifest_arch, "arm64");

        let x86_64 = android_target(Some("android-x86_64")).unwrap();
        assert_eq!(x86_64.rust_triple, "x86_64-linux-android");
        assert_eq!(x86_64.clang_target, "x86_64-linux-android24");
        assert_eq!(x86_64.manifest_arch, "x64");

        assert_eq!(android_target(Some("wearos")), Some(arm64));
    }

}
