//! Elixir toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted Elixir project and runs `mix compile` in it.
//! Controllers + router are excluded via mix.exs's `elixirc_paths`
//! filter.
//!
//! Marked `#[ignore]` since first-ever run downloads Elixir's
//! build artifacts. Explicit invocation:
//!
//!     cargo test --test elixir_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::elixir;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-elixir-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = elixir::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn assert_mix_compile_passes(fixture: &str, scratch: &Path) {
    let output = Command::new("mix")
        .arg("compile")
        .arg("--warnings-as-errors")
        .current_dir(scratch)
        .output()
        .expect("run mix compile");

    assert!(
        output.status.success(),
        "mix compile failed on emitted {fixture} project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn tiny_blog_mix_compile_passes() {
    let fixture = Path::new("fixtures/tiny-blog");
    let scratch = scratch_dir("tiny-blog");
    generate_project(fixture, &scratch);
    assert_mix_compile_passes("tiny-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_mix_compile_passes() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    assert_mix_compile_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_mix_test_passes() {
    // Phase 2 forcing function: emit real-blog, run `mix test`,
    // assert zero failures. Phase-3 tests are tagged `:skip`.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("mix")
        .arg("test")
        .current_dir(&scratch)
        .output()
        .expect("run mix test");

    assert!(
        output.status.success(),
        "mix test failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
