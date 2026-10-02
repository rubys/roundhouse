//! A file that cannot be read (in practice: a symlink whose target is
//! absent from the checkout — Procore's
//! `app/views/shared/_princess_footer.pdf.erb` points into a
//! `components/` package that isn't always checked out alongside it)
//! used to propagate an `io error: No such file or directory` through
//! the ERB walk's `vfs.read_to_string(&erb_path)?` and abort the
//! ENTIRE app ingest over one bad file (#F35).
//!
//! `MapVfs` cannot model a dangling symlink (it's a flat path→bytes
//! map with no such concept), so this exercises `ingest_app` against a
//! REAL temp directory built with `std::fs`.

use std::path::{Path, PathBuf};

fn unique_tmp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("roundhouse_{name}_{}_{}", std::process::id(), std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()));
    dir
}

/// A minimal Rails app tree with one readable view and one dangling
/// symlink sitting right next to it in the same directory — the exact
/// shape of the bug: a walk over `app/views/**` that hits a good file
/// and a broken one side by side.
fn build_app_with_dangling_symlink(dir: &Path) {
    let views_dir = dir.join("app/views/widgets");
    std::fs::create_dir_all(&views_dir).expect("create views dir");
    std::fs::write(views_dir.join("index.html.erb"), "<p>Widgets</p>\n").expect("write readable erb");
    #[cfg(unix)]
    std::os::unix::fs::symlink("does_not_exist_anywhere", views_dir.join("_broken.html.erb"))
        .expect("create dangling symlink");
}

#[cfg(unix)]
#[test]
fn survey_mode_skips_an_unreadable_file_and_ledgers_it() {
    let dir = unique_tmp_dir("survey");
    build_app_with_dangling_symlink(&dir);

    roundhouse::ingest::survey::activate();
    let app = roundhouse::ingest::ingest_app(&dir).expect("survey mode never aborts on one bad file");
    let gaps = roundhouse::ingest::survey::drain();

    // The good file next to the broken one still ingested.
    assert!(
        app.views.iter().any(|v| v.name.as_str() == "widgets/index"),
        "the readable view should still ingest: {:?}",
        app.views.iter().map(|v| v.name.as_str().to_string()).collect::<Vec<_>>()
    );

    // The unreadable file is ledgered — visible, not silently dropped.
    let messages: Vec<String> = gaps.iter().map(|g| format!("{g}")).collect();
    assert!(
        messages.iter().any(|m| m.contains("file not readable") && m.contains("_broken.html.erb")),
        "{messages:?}"
    );

    std::fs::remove_dir_all(&dir).expect("clean up temp dir");
}

#[cfg(unix)]
#[test]
fn strict_mode_still_errors_on_an_unreadable_file() {
    let dir = unique_tmp_dir("strict");
    build_app_with_dangling_symlink(&dir);

    // No survey::activate() here — strict mode is the default.
    let err = roundhouse::ingest::ingest_app(&dir).expect_err("strict mode must still surface the gap");
    assert!(err.to_string().contains("file not readable"), "{err}");

    std::fs::remove_dir_all(&dir).expect("clean up temp dir");
}
