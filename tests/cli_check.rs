//! `roundhouse check` at the process boundary: the exit code is the
//! contract a CI gate reads, so the cases that must NOT report clean
//! are pinned here.

use std::process::Command;

fn check(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .arg("check")
        .args(args)
        .output()
        .expect("spawn roundhouse");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_missing_path_is_not_a_clean_app() {
    let (code, err) = check(&["/nonexistent/rails/app"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("is not a directory"), "{err}");
    assert!(!err.contains("0 error(s)"), "must not print a summary: {err}");
}

#[test]
fn a_directory_without_app_is_not_a_clean_app() {
    // The repo root: a directory, but not a Rails app.
    let (code, err) = check(&[env!("CARGO_MANIFEST_DIR")]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("does not look like a Rails app"), "{err}");
}

#[test]
fn the_store_fixture_checks_clean() {
    let store = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/store");
    let (code, err) = check(&[store]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("0 parse error(s), 0 error(s), 0 warning(s), 0 gap-attributed note(s), 0 survey gap(s)"),
        "{err}"
    );
}
