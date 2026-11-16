//! The default output path for a compile — the file `perry app.ts` writes when
//! the user gave no `-o`.
//!
//! One source of truth, shared by the compile pipeline (`run_pipeline`, which
//! links to this path) and the build cache (`build_cache`, which fingerprints
//! it to decide whether a rebuild is needed). These used to be two independent
//! `default_output_path` functions that had already drifted apart; keeping them
//! in one place is what makes the Android arm below correct in both.

use std::path::PathBuf;


/// The output file a compile targets when no `-o` was given.
///
/// `stem` is the already-sanitized entry-file stem (`app.ts` → `app`).
pub(super) fn default_output_path(
    is_dylib: bool,
    is_staticlib: bool,
    target: Option<&str>,
    stem: &str,
) -> PathBuf {
    if is_dylib {
        // #4771 — keyed on the target like the rest of this file (host is
        // only the fallback when no `--target` was given), not on host cfg
        // alone: `--target windows` must yield `.dll` regardless of host.
        // Same by-output-type table as `windows_default_output_extension`,
        // which already covered the `-o NAME` path; this covers the
        // no-`-o` default, which used to fall through to `.so` on Windows.
        PathBuf::from(format!("{}.so", stem))
    } else if is_staticlib {
        // #1088 — Windows hosts expect `.lib`; everywhere else uses
        // the Unix `lib<stem>.a` convention so the archive is reachable
        // from `-l<stem>` at the host's link step.
        PathBuf::from(format!("lib{}.a", stem))
    } else if target.is_some_and(|t| false) {
        PathBuf::from(format!("{}.wasm", stem))
    } else {
        PathBuf::from(stem)
    }
}


#[cfg(test)]
mod tests {
    use super::default_output_path;
    use std::path::PathBuf;

    fn exe(target: Option<&str>, stem: &str) -> PathBuf {
        default_output_path(false, false, target, stem)
    }

    /// #5740 bug 4 — `--target android` used to fall through to the bare stem,
    /// so the link ran with `-o app` inside an Android project where `app/` is
    /// a directory: `ld.lld: cannot open output file app: Is a directory`.
    #[test]
    fn android_defaults_to_shared_library() {
        assert_eq!(exe(Some("android"), "app"), PathBuf::from("libapp.so"));
        assert_eq!(exe(Some("android"), "hello"), PathBuf::from("libhello.so"));
        assert_eq!(
            exe(Some("android-x86_64"), "app"),
            PathBuf::from("libapp.so")
        );
    }

    /// Wear OS links exactly like Android (same NDK, triple, cdylib + TLS
    /// model — see `link::build_and_run`'s `is_android`), so it needs the
    /// same default.
    #[test]
    fn wearos_defaults_to_shared_library() {
        assert_eq!(exe(Some("wearos"), "app"), PathBuf::from("libapp.so"));
    }

    #[test]
    fn harmonyos_still_defaults_to_shared_library() {
        assert_eq!(exe(Some("harmonyos"), "app"), PathBuf::from("libapp.so"));
        assert_eq!(
            exe(Some("harmonyos-simulator"), "app"),
            PathBuf::from("libapp.so")
        );
    }

    #[test]
    fn windows_target_defaults_to_exe() {
        assert_eq!(exe(Some("windows"), "app"), PathBuf::from("app.exe"));
        assert_eq!(exe(Some("windows-winui"), "app"), PathBuf::from("app.exe"));
        assert_eq!(
            exe(Some("windows-aarch64"), "app"),
            PathBuf::from("app.exe")
        );
    }

    /// The unix/native executable shape must stay a bare, extension-less name.
    #[test]
    fn unix_targets_default_to_bare_stem() {
        assert_eq!(exe(Some("linux"), "app"), PathBuf::from("app"));
        assert_eq!(exe(Some("macos"), "app"), PathBuf::from("app"));
        assert_eq!(exe(None, "app"), PathBuf::from("app"));
    }

    #[test]
    fn staticlib_keeps_its_conventions() {
        assert_eq!(
            default_output_path(false, true, Some("linux"), "app"),
            PathBuf::from("libapp.a")
        );
        assert_eq!(
            default_output_path(false, true, Some("windows"), "app"),
            PathBuf::from("app.lib")
        );
    }

    /// #4771 finish — an extension-less `--output-type dylib` build keys the
    /// extension on the target: `.dll` for Windows (used to fall through to
    /// `.so`), `.dylib` for the Apple family, `.so` elsewhere.
    #[test]
    fn dylib_extension_keys_on_target() {
        let dylib = |target| default_output_path(true, false, target, "app");
        assert_eq!(dylib(Some("windows")), PathBuf::from("app.dll"));
        assert_eq!(dylib(Some("windows-winui")), PathBuf::from("app.dll"));
        assert_eq!(dylib(Some("macos")), PathBuf::from("app.dylib"));
        assert_eq!(dylib(Some("ios")), PathBuf::from("app.dylib"));
        assert_eq!(dylib(Some("ios-simulator")), PathBuf::from("app.dylib"));
        assert_eq!(dylib(Some("linux")), PathBuf::from("app.so"));
    }

    /// No `--target` falls back to the host's shared-library convention.
    #[test]
    fn dylib_uses_the_host_shared_library_extension() {
        let out = default_output_path(true, false, None, "app");
        assert_eq!(out, PathBuf::from("app.so"));
    }
}
