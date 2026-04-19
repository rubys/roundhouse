//! Python toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted Python project and runs `python -m compileall`
//! over the models package to verify syntax. Python's dynamic
//! late-binding means undefined names don't fail compile (they fail at
//! run time), so compileall is the honest pre-test check available
//! without third-party tools.
//!
//! Marked `#[ignore]`. Explicit:
//!
//!     cargo test --test python_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::python;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-python-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = python::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn assert_python_compiles(fixture: &str, scratch: &Path) {
    // Only compile-check the models package; controllers/routes
    // reference a runtime we haven't wired yet.
    let output = Command::new("python3")
        .arg("-m")
        .arg("compileall")
        .arg("-q")
        .arg("app/models.py")
        .current_dir(scratch)
        .output()
        .expect("run python compileall");

    assert!(
        output.status.success(),
        "python compileall failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn tiny_blog_python_compile_passes() {
    let fixture = Path::new("fixtures/tiny-blog");
    let scratch = scratch_dir("tiny-blog");
    generate_project(fixture, &scratch);
    assert_python_compiles("tiny-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_python_compile_passes() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    assert_python_compiles("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_python_unittest_passes() {
    // Phase 2 forcing function: emit real-blog, run `python -m
    // unittest discover tests`, assert zero failures. Phase-3 tests
    // are @unittest.skip'd.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("python3")
        .arg("-m")
        .arg("unittest")
        .arg("discover")
        .arg("-s")
        .arg("tests")
        .arg("-t")
        .arg(".")
        .current_dir(&scratch)
        .output()
        .expect("run python unittest");

    assert!(
        output.status.success(),
        "python unittest failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
