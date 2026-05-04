//! TypeScript toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted TypeScript project for a fixture into a
//! scratch directory, runs `npx tsc --noEmit` against it, asserts
//! zero errors. `--noEmit` performs parse + type-check without
//! writing JS output — the TS equivalent of `cargo check` /
//! `crystal build --no-codegen`.
//!
//! Marked `#[ignore]` because the first run on a machine without a
//! cached TypeScript install can take ~30s while npx pulls the
//! package. Run explicitly:
//!
//!     cargo test --test typescript_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::typescript;
use roundhouse::ingest::ingest_app;

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-ts-check-{tag}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = typescript::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn assert_tsc_passes(fixture: &str, scratch: &Path) {
    // Install declared devDependencies (`@types/node`) so tsc can
    // resolve `node:test` / `node:assert/strict` in the emitted specs.
    let install = Command::new("npm")
        .arg("install")
        .arg("--silent")
        .arg("--no-audit")
        .arg("--no-fund")
        .current_dir(scratch)
        .output()
        .expect("run npm install");
    assert!(
        install.status.success(),
        "npm install failed for {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    // Invoke the locally-installed tsc directly — `npx tsc` resolves
    // to a typosquatting-prevention stub, and `npx --package=` can
    // get confused by the node_modules we just populated.
    let output = Command::new("./node_modules/.bin/tsc")
        .arg("-p")
        .arg(".")
        .arg("--noEmit")
        .current_dir(scratch)
        .output()
        .expect("run tsc");

    assert!(
        output.status.success(),
        "tsc failed on emitted {fixture} project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn tiny_blog_tsc_passes() {
    let fixture = Path::new("fixtures/tiny-blog");
    let scratch = scratch_dir("tiny-blog");
    generate_project(fixture, &scratch);
    assert_tsc_passes("tiny-blog", &scratch);
}

/// Diagnostic test: emit transpiled_blog and dump tsc errors so we
/// know what the body-walker still needs to fix. Does NOT assert
/// success — purely an inspection helper.
#[test]
#[ignore]
fn dump_transpiled_blog_tsc_errors() {
    let fixture = Path::new("runtime/ruby/test/fixtures/transpiled_blog");
    let scratch = scratch_dir("transpiled-blog-tsc-dump");
    generate_project(fixture, &scratch);

    let install = Command::new("npm")
        .arg("install")
        .arg("--silent")
        .arg("--no-audit")
        .arg("--no-fund")
        .current_dir(&scratch)
        .output()
        .expect("run npm install");
    if !install.status.success() {
        eprintln!(
            "npm install failed:\n{}",
            String::from_utf8_lossy(&install.stderr)
        );
        return;
    }

    let output = Command::new("./node_modules/.bin/tsc")
        .arg("-p")
        .arg(".")
        .arg("--noEmit")
        .current_dir(&scratch)
        .output()
        .expect("run tsc");

    println!("=== tsc exit status: {} ===", output.status);
    println!("=== stdout ===\n{}", String::from_utf8_lossy(&output.stdout));
    println!("=== stderr ===\n{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore]
fn real_blog_tsc_passes() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-tsc");
    generate_project(fixture, &scratch);
    assert_tsc_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_node_test_passes() {
    // Phase 2 forcing function: emit real-blog, run `node:test` via
    // tsx (for TS transpile) against the emitted spec files, assert
    // zero failures. Mirrors the Rust/Crystal Phase 2 bar —
    // Phase-3-dependent tests are marked `test.skip(...)`.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-node");
    generate_project(fixture, &scratch);

    // Shell invocation so glob expansion picks up every test file —
    // passing the directory confuses tsx/node:test when there's no
    // index entry. tsx registers a module loader so `.ts` imports
    // resolve at run time.
    let install = Command::new("npm")
        .arg("install")
        .arg("--silent")
        .arg("--no-audit")
        .arg("--no-fund")
        .current_dir(&scratch)
        .output()
        .expect("run npm install");
    assert!(
        install.status.success(),
        "npm install failed for real-blog node test at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    let output = Command::new("sh")
        .arg("-c")
        .arg("./node_modules/.bin/tsx --test test/*.test.ts")
        .current_dir(&scratch)
        .output()
        .expect("run node --test via tsx");

    assert!(
        output.status.success(),
        "node --test failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
