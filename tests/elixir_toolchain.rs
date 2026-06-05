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

/// Generate the project via the `roundhouse` binary in a child process,
/// with the elixir2 overlay gates (`RH_ELIXIR2_{MODELS,VIEWS,CONTROLLERS}`)
/// set on that child only. The gates are read with `std::env::var` inside
/// `emit_overlay_files`, so setting them in *this* process would leak into
/// the parallel in-process v1 tests' `elixir::emit` calls and make them
/// emit (and fail to compile) the v2 tree. Shelling out isolates them.
fn generate_project_v2(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .arg("--target")
        .arg("elixir")
        .arg(fixture_path)
        .arg("-o")
        .arg(out)
        .env("RH_ELIXIR2_MODELS", "1")
        .env("RH_ELIXIR2_VIEWS", "1")
        .env("RH_ELIXIR2_CONTROLLERS", "1")
        .output()
        .expect("run roundhouse --target elixir");
    assert!(
        output.status.success(),
        "roundhouse emit (v2 gates on) failed for {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        fixture_path.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn mix_deps_get(scratch: &Path) {
    let output = Command::new("mix")
        .arg("deps.get")
        .current_dir(scratch)
        .env("MIX_ENV", "test")
        // Hex package fetches need a cache; isolate per-test to
        // avoid parallel `mix local.hex` races on fresh runners.
        .env("MIX_HOME", scratch.join(".mix"))
        .output()
        .expect("run mix deps.get");
    assert!(
        output.status.success(),
        "mix deps.get failed at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn assert_mix_compile_passes(fixture: &str, scratch: &Path) {
    mix_deps_get(scratch);
    let output = Command::new("mix")
        .arg("compile")
        .arg("--warnings-as-errors")
        .current_dir(scratch)
        .env("MIX_HOME", scratch.join(".mix"))
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
    let scratch = scratch_dir("real-blog-compile");
    generate_project(fixture, &scratch);
    assert_mix_compile_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_v2_overlay_mix_compile_passes() {
    // elixir2 strangler forcing function: emit real-blog with the full
    // v2 overlay on (models + views + controllers + dispatch/main/server)
    // and assert the emitted project mix-compiles clean under
    // `--warnings-as-errors`. This is the end-to-end gate the per-layer
    // `emit_library_class` unit tests can't give — it proves the whole
    // overlay links and compiles as one app, the precondition for byte
    // parity vs Rails and the eventual v1 switchover. real-blog has no
    // associations, so the open `has_many` recursion-lowering gap doesn't
    // apply here — it's the first fixture expected green.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-v2-compile");
    generate_project_v2(fixture, &scratch);
    assert_mix_compile_passes("real-blog (v2 overlay)", &scratch);
}

#[test]
#[ignore]
fn real_blog_mix_test_passes() {
    // Phase 2 forcing function: emit real-blog, run `mix test`,
    // assert zero failures. Phase-3 tests are tagged `:skip`.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-test");
    generate_project(fixture, &scratch);
    mix_deps_get(&scratch);

    let output = Command::new("mix")
        .arg("test")
        .current_dir(&scratch)
        .env("MIX_HOME", scratch.join(".mix"))
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
