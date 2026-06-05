//! Elixir toolchain integration test — emit forcing function.
//!
//! Generates the emitted Elixir project and runs `mix compile` in it.
//! As of Phase D the v2 overlay (`V2.*`) emits unconditionally, so a
//! default emit covers both the legacy v1 app shell and the v2 stack;
//! these tests compile (and `mix test`) the whole tree.
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

#[test]
#[ignore]
fn real_blog_v2_mix_test_passes() {
    // Phase D2 / W7 forcing function: run ONLY the v2 ExUnit tree
    // (`test/v2/**`, driving the V2.* stack via V2.TestClient) and assert
    // zero failures. `real_blog_mix_test_passes` already runs these as
    // part of the whole suite; this names the v2 coverage as its own gate
    // (and is the test that survives v1 deletion in D3).
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-v2-test");
    generate_project(fixture, &scratch);
    mix_deps_get(&scratch);

    let output = Command::new("mix")
        .arg("test")
        .arg("test/v2")
        .current_dir(&scratch)
        .env("MIX_HOME", scratch.join(".mix"))
        .output()
        .expect("run mix test test/v2");

    assert!(
        output.status.success(),
        "mix test test/v2 failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
