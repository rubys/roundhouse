//! Rust toolchain integration test — the Phase 1 forcing function.
//!
//! Generates the emitted Rust project for a fixture into a scratch
//! directory and runs `cargo check` against it. "Zero errors" means
//! the emitter's output is syntactically valid Rust that the compiler
//! accepts at the structural level — not yet that it runs, just that
//! it compiles.
//!
//! Scoped to `tiny-blog` for Phase 1. Controllers are emitted as files
//! but not declared in `src/lib.rs` (they reference runtime the
//! generated code doesn't have yet). When Phase 2 lands, the scope
//! extends to real-blog + `cargo test` on the model tests.
//!
//! Marked `#[ignore]` so the default `cargo test` run stays fast —
//! this test shells out to cargo itself and is slow (multi-second) on
//! a cold scratch dir. Run explicitly with:
//!
//!     cargo test --test rust_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-rust-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = rust::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

#[test]
#[ignore]
fn tiny_blog_cargo_check_passes() {
    let fixture = Path::new("fixtures/tiny-blog");
    let scratch = scratch_dir("tiny-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("cargo")
        .arg("check")
        .arg("--quiet")
        .current_dir(&scratch)
        .output()
        .expect("run cargo check");

    assert!(
        output.status.success(),
        "cargo check failed on emitted tiny-blog project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_cargo_test_passes() {
    // Phase 2b forcing function: emit real-blog, run cargo test
    // against the generated project, assert the non-ignored model
    // tests pass. Two tests are marked #[ignore] because they need
    // persistence runtime (Phase 3); the rest should pass cleanly.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("cargo")
        .arg("test")
        .arg("--quiet")
        .current_dir(&scratch)
        .output()
        .expect("run cargo test");

    assert!(
        output.status.success(),
        "cargo test failed on emitted real-blog project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
