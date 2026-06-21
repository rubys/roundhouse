//! C# toolchain integration test — compile the full real-blog emit.
//!
//! Generates the emitted C# (ASP.NET Core) project for a fixture into a
//! scratch dir and runs `dotnet build` against it — the compile/typecheck
//! gate, the C# analog of `go vet` / `crystal build --no-codegen` / `tsc
//! --noEmit`. Catches emit regressions (the model layer, the transpiled
//! framework runtime, and the hand-written primitives all compile together).
//!
//! Marked `#[ignore]` so the default `cargo test` run stays fast; run with:
//!
//!     cargo test --test csharp_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::csharp;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-csharp-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = csharp::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn assert_dotnet_build_passes(fixture: &str, scratch: &Path) {
    // `dotnet build` does an implicit restore, so this is a single
    // self-contained compile of the whole emitted project.
    let output = Command::new("dotnet")
        .arg("build")
        .arg("--nologo")
        .current_dir(scratch)
        .output()
        .expect("run dotnet build");

    assert!(
        output.status.success(),
        "dotnet build failed on emitted {fixture} project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_dotnet_build_passes() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    assert_dotnet_build_passes("real-blog", &scratch);
}
