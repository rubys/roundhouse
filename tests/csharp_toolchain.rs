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

/// The execution half of the toolchain gate: emit real-blog and run the
/// transpiled suite under `dotnet test` — ArticleTest 4, CommentTest 5,
/// ArticlesControllerTest 9, CommentsControllerTest 3 = 21, executed
/// against a per-test SQLite database. Sibling of
/// `real_blog_kotlin_tests_pass` / `real_blog_swift_tests_pass`.
///
/// The **executed-count floor** is the point: `dotnet test` exits 0 when it
/// discovers nothing, which is exactly how the csharp gate reported success
/// while running zero assertions before this suite existed (#34 §0). Parsing
/// the summary and asserting `>= 21` is what makes a silent regression —
/// emit stops producing tests, or the xUnit runner package drops out — fail.
#[test]
#[ignore]
fn real_blog_csharp_tests_pass() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-tests");
    generate_project(fixture, &scratch);

    let output = Command::new("dotnet")
        .arg("test")
        .arg("tests")
        .arg("--nologo")
        .current_dir(&scratch)
        .output()
        .expect("run dotnet test");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(
        output.status.success(),
        "dotnet test failed on emitted real-blog project at {}:\n\
         \n=== stdout ===\n{stdout}\n\
         \n=== stderr ===\n{stderr}",
        scratch.display(),
    );

    let passed = parse_passed_count(&stdout).unwrap_or_else(|| {
        panic!(
            "no `Passed: N` summary in dotnet test output — the runner reported \
             nothing, which is the zero-discovery trap this gate exists to catch \
             (#34 §0):\n{stdout}"
        )
    });
    assert!(
        passed >= 21,
        "expected at least 21 executed tests (ArticleTest 4 + CommentTest 5 + \
         ArticlesControllerTest 9 + CommentsControllerTest 3), got {passed}:\n{stdout}"
    );
    eprintln!("csharp real-blog suite: {passed} tests passed");
}

/// `Passed!  - Failed:     0, Passed:    21, …` → 21.
fn parse_passed_count(stdout: &str) -> Option<usize> {
    for line in stdout.lines() {
        let Some(idx) = line.find("Passed:") else { continue };
        let rest = line[idx + "Passed:".len()..].trim_start();
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<usize>() {
            return Some(n);
        }
    }
    None
}

#[test]
fn parses_the_dotnet_test_summary() {
    let line = "Passed!  - Failed:     0, Passed:    21, Skipped:     0, Total:    21";
    assert_eq!(parse_passed_count(line), Some(21));
    // The `Passed!` banner alone (no counts) must NOT read as a count.
    assert_eq!(parse_passed_count("Build succeeded."), None);
}
