//! Go toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted Go project for a fixture into a scratch dir
//! and runs `go vet ./app` against it. Go's `vet` parses + type-checks
//! the package without linking — closest equivalent to `cargo check` /
//! `crystal build --no-codegen` / `tsc --noEmit`.
//!
//! Scoped to the `./app` package, which as of Phase 4c contains
//! controllers, http runtime, and models all flat in one package.
//!
//! Marked `#[ignore]`; run with:
//!
//!     cargo test --test go_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::go;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-go-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = go::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn go_mod_tidy(scratch: &Path) {
    let tidy = Command::new("go")
        .arg("mod")
        .arg("tidy")
        .current_dir(scratch)
        .output()
        .expect("run go mod tidy");
    assert!(
        tidy.status.success(),
        "go mod tidy failed at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&tidy.stdout),
        String::from_utf8_lossy(&tidy.stderr),
    );
}

fn assert_go_passes(fixture: &str, scratch: &Path) {
    go_mod_tidy(scratch);
    let output = Command::new("go")
        .arg("vet")
        .arg("./app")
        .current_dir(scratch)
        .output()
        .expect("run go vet");

    assert!(
        output.status.success(),
        "go vet failed on emitted {fixture} project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_go_vet_passes() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-vet");
    generate_project(fixture, &scratch);
    assert_go_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn tiny_blog_go_vet_passes() {
    let fixture = Path::new("fixtures/tiny-blog");
    let scratch = scratch_dir("tiny-blog");
    generate_project(fixture, &scratch);
    assert_go_passes("tiny-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_go_test_passes() {
    // Phase 2 forcing function: emit real-blog, run `go test`
    // against the generated package, assert zero failures.
    // Phase-3-dependent tests are `t.Skip`-ped.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-test");
    generate_project(fixture, &scratch);
    go_mod_tidy(&scratch);

    let output = Command::new("go")
        .arg("test")
        .arg("./app")
        .current_dir(&scratch)
        .output()
        .expect("run go test");

    assert!(
        output.status.success(),
        "go test failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
