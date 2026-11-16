use super::{host_target_triple, locate_native_lib_artifact};
use std::fs;

/// Refs #564 — when cargo writes to `target/<triple>/release/`
/// (because something pinned a default target), perry must still
/// find the artifact for a native build (no `--target` passed).
#[test]
fn locates_artifact_under_host_triple_dir_for_native_build() {
    let host = match host_target_triple() {
        Some(h) => h,
        None => return, // rustc unavailable in this test env — skip.
    };

    let tmp = tempfile::tempdir().expect("create tmpdir");
    let target_dir = tmp.path().join("target");
    let triple_dir = target_dir.join(host).join("release");
    fs::create_dir_all(&triple_dir).expect("mkdir triple/release");
    let lib_path = triple_dir.join("libfoo.a");
    fs::write(&lib_path, b"fake archive").expect("write lib");

    let found = locate_native_lib_artifact(&target_dir, None, "libfoo.a");
    assert_eq!(found.as_deref(), Some(lib_path.as_path()));
}

#[test]
fn prefers_bare_release_dir_when_present() {
    let tmp = tempfile::tempdir().expect("create tmpdir");
    let target_dir = tmp.path().join("target");
    let release_dir = target_dir.join("release");
    fs::create_dir_all(&release_dir).expect("mkdir release");
    let lib_path = release_dir.join("libfoo.a");
    fs::write(&lib_path, b"fake archive").expect("write lib");

    let found = locate_native_lib_artifact(&target_dir, None, "libfoo.a");
    assert_eq!(found.as_deref(), Some(lib_path.as_path()));
}

/// Refs #792 — wrappers that supply only the cargo crate name
/// (e.g. `perry_ext_foo`) instead of the full filename should
/// still resolve to `libperry_ext_foo.a` on the host platform.
#[test]
fn locates_artifact_from_bare_crate_name() {
    let tmp = tempfile::tempdir().expect("create tmpdir");
    let target_dir = tmp.path().join("target");
    let release_dir = target_dir.join("release");
    fs::create_dir_all(&release_dir).expect("mkdir release");
    let lib_name = "libperry_ext_foo.a";
    let lib_path = release_dir.join(lib_name);
    fs::write(&lib_path, b"fake archive").expect("write lib");

    let found = locate_native_lib_artifact(&target_dir, None, "perry_ext_foo");
    assert_eq!(found.as_deref(), Some(lib_path.as_path()));
}




/// The `.a` → `.lib` translation is Windows-only; a Unix target must
/// not start probing for `.lib` files.
#[test]
fn variant_set_does_not_translate_static_lib_on_unix() {
    let variants = super::lib_name_variants("libperry_ext_webgpu.a");
    assert_eq!(variants, vec!["libperry_ext_webgpu.a".to_string()]);
}


#[test]
fn returns_none_when_artifact_missing() {
    let tmp = tempfile::tempdir().expect("create tmpdir");
    let target_dir = tmp.path().join("target");
    fs::create_dir_all(&target_dir).expect("mkdir target");
    let found = locate_native_lib_artifact(&target_dir, None, "libfoo.a");
    assert!(found.is_none());
}

/// The host triple is the compiler's own build target: a compile with a
/// native addon must not need `rustc` on `PATH` to name it.
#[test]
fn host_triple_does_not_need_rustc_on_path() {
    let empty = tempfile::tempdir().expect("create tmpdir");
    let saved = std::env::var_os("PATH");
    std::env::set_var("PATH", empty.path());
    let host = host_target_triple();
    match saved {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }
    assert_eq!(host, Some(env!("PERRY_HOST_TRIPLE")));
}
