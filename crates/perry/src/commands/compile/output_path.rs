//! The default output path for a compile — the file `perry app.ts` writes when
//! the user gave no `-o`.
//!
//! One source of truth, shared by the compile pipeline (`run_pipeline`, which
//! links to this path) and the build cache (`build_cache`, which fingerprints
//! it to decide whether a rebuild is needed). These used to be two independent
//! `default_output_path` functions that had already drifted apart.

use std::path::PathBuf;


/// The output file a compile targets when no `-o` was given.
///
/// `stem` is the already-sanitized entry-file stem (`app.ts` → `app`).
pub(super) fn default_output_path(
    is_dylib: bool,
    is_staticlib: bool,
    stem: &str,
) -> PathBuf {
    if is_dylib {
        PathBuf::from(format!("{}.so", stem))
    } else if is_staticlib {
        // #1088 — the Unix `lib<stem>.a` convention, so the archive is
        // reachable from `-l<stem>` at the host's link step.
        PathBuf::from(format!("lib{}.a", stem))
    } else {
        PathBuf::from(stem)
    }
}


#[cfg(test)]
mod tests {
    use super::default_output_path;
    use std::path::PathBuf;

    /// An executable is a bare name with no extension.
    #[test]
    fn an_executable_defaults_to_the_bare_stem() {
        assert_eq!(default_output_path(false, false, "app"), PathBuf::from("app"));
    }

    #[test]
    fn a_static_library_is_lib_stem_dot_a() {
        assert_eq!(
            default_output_path(false, true, "app"),
            PathBuf::from("libapp.a")
        );
    }

    #[test]
    fn a_shared_library_is_stem_dot_so() {
        let out = default_output_path(true, false, "app");
        assert_eq!(out, PathBuf::from("app.so"));
    }
}
