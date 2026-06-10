//! Swift toolchain integration test — emit-then-compile forcing function.
//!
//! Generates the emitted Swift/SPM project for real-blog into a scratch
//! directory and runs `swift build` against it — the Swift analog of
//! `kotlin_toolchain.rs`'s `gradle compileKotlin`: full parse + type-check
//! (+ link) of every emitted source against the real Hummingbird /
//! swift-nio / CSQLite dependency graph. Bare `swiftc` can't be used
//! because `Server.swift` imports Hummingbird and `Db.swift` imports
//! NIOPosix + the CSQLite systemLibrary, so the build has to go through
//! SPM (which resolves the declared dependencies).
//!
//! Requires a Swift toolchain (6+) on PATH and, on Linux, the
//! `libsqlite3-dev` package (the CSQLite systemLibrary's header + link
//! target — see docs/swift-migration-plan.md decision 3). CI provides
//! both via `swift-actions/setup-swift` + apt; locally, the Xcode CLT
//! (macOS) or a swiftly/mise toolchain suffices.
//!
//! Marked `#[ignore]` so the default `cargo test` run stays fast. Run:
//!
//!     cargo test --test swift_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::swift;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-swift-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    // Keep `.build/` across runs so SPM's resolved checkouts and compiled
    // dependency modules are reused locally (the emitted app sources are
    // tiny next to the Hummingbird/swift-nio tree); everything else is
    // regenerated from scratch.
    if out.exists() {
        for entry in std::fs::read_dir(out).expect("read scratch") {
            let entry = entry.expect("scratch entry");
            if entry.file_name() == ".build" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path).expect("clean scratch entry");
            } else {
                std::fs::remove_file(&path).expect("clean scratch file");
            }
        }
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = swift::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

/// `swift build` over the emitted project (debug configuration — the
/// type-check + link is the gate; release codegen adds minutes for no
/// extra verification).
fn run_swift_build(scratch: &Path) -> std::process::Output {
    let mut cmd = Command::new("swift");
    cmd.arg("build").current_dir(scratch);
    cmd.output().expect("run swift build")
}

fn assert_swift_builds(fixture: &str, scratch: &Path) {
    let output = run_swift_build(scratch);
    assert!(
        output.status.success(),
        "swift build failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_swift_builds() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    assert_swift_builds("real-blog", &scratch);
}

/// Execution leg: `swift test` over the emitted project runs the
/// TRANSPILED real-blog suite (article/comment model tests + controller
/// dispatch tests) under XCTest against an in-memory SQLite database —
/// compile-clean is necessary but not sufficient; this catches runtime
/// contract drift (fixture loading, Router.match dispatch, covariant
/// `class func` dispatch through inherited Base bodies).
///
/// Same prerequisites as the build leg PLUS XCTest, which Linux
/// toolchains bundle but the macOS Command Line Tools do NOT. On macOS,
/// point at a full Xcode per-invocation (no xcode-select needed):
///
///     DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
///         cargo test --test swift_toolchain -- --ignored --nocapture
#[test]
#[ignore]
fn real_blog_swift_tests_pass() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-test");
    generate_project(fixture, &scratch);

    let output = Command::new("swift")
        .arg("test")
        .current_dir(&scratch)
        .output()
        .expect("run swift test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "swift test failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        stdout,
        stderr,
    );

    // Defense against issue #4: `swift test` exits 0 when zero XCTest
    // methods are discovered. real-blog carries 21 tests across 4
    // suites; require a healthy floor so emit-routing can't silently
    // drop a whole test class.
    let executed = parse_executed_count(&stdout).or_else(|| parse_executed_count(&stderr));
    assert!(
        executed.map_or(false, |n| n >= 21),
        "expected >= 21 real-blog tests to run, got {executed:?}\n\
         stdout:\n{stdout}\nstderr:\n{stderr}",
    );
}

/// Extract N from the LAST XCTest "Executed N test(s)" summary line (the
/// all-tests rollup).
fn parse_executed_count(s: &str) -> Option<usize> {
    let idx = s.rfind("Executed ")?;
    let rest = &s[idx + "Executed ".len()..];
    let end = rest.find(' ')?;
    rest[..end].parse::<usize>().ok()
}
