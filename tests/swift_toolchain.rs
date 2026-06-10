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
