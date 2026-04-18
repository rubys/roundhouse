//! Crystal toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted Crystal project for tiny-blog into a scratch
//! directory and runs `crystal build --no-codegen` against it. The
//! `--no-codegen` flag performs parsing + semantic analysis + type
//! checking without producing a binary, which is the Crystal
//! equivalent of `cargo check` — fast enough to be useful in a
//! test, and catches the same class of emit bugs.
//!
//! Marked `#[ignore]` so the default `cargo test` run stays fast.
//! Run explicitly:
//!
//!     cargo test --test crystal_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::crystal;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-crystal-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = crystal::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn run_crystal_build(scratch: &Path) -> std::process::Output {
    Command::new("crystal")
        .arg("build")
        .arg("--no-codegen")
        .arg("src/app.cr")
        .current_dir(scratch)
        .output()
        .expect("run crystal build")
}

fn assert_crystal_passes(fixture: &str, scratch: &Path) {
    let output = run_crystal_build(scratch);
    assert!(
        output.status.success(),
        "crystal build --no-codegen failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn tiny_blog_crystal_build_passes() {
    let fixture = Path::new("fixtures/tiny-blog");
    let scratch = scratch_dir("tiny-blog");
    generate_project(fixture, &scratch);
    assert_crystal_passes("tiny-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_crystal_build_passes() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    assert_crystal_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_crystal_spec_passes() {
    // Phase 2 forcing function: emit real-blog, run `crystal spec`
    // against the generated project, assert zero failures. A subset
    // of tests are marked `pending` because they need persistence
    // runtime (Phase 3); the rest should pass.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("crystal")
        .arg("spec")
        .arg("--no-color")
        .current_dir(&scratch)
        .output()
        .expect("run crystal spec");

    assert!(
        output.status.success(),
        "crystal spec failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
