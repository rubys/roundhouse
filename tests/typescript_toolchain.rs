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

fn generate_project_with_profile(
    fixture_path: &Path,
    out: &Path,
    profile: &roundhouse::profile::DeploymentProfile,
) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = typescript::emit_with_profile(&app, profile);

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

/// libsql (`node-async`) profile: emit real-blog with async coloring
/// active, type-check the result. Forcing function for the libsql
/// runtime + async-emit pipeline — every regression in the
/// runtime↔app propagation handshake or the emit-time await-wrap
/// filters surfaces here as a tsc error.
#[test]
#[ignore]
fn real_blog_libsql_tsc_passes() {
    use roundhouse::profile::DeploymentProfile;

    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-libsql-tsc");
    generate_project_with_profile(fixture, &scratch, &DeploymentProfile::node_async());
    assert_tsc_passes("real-blog (libsql)", &scratch);
}

fn assert_node_test_passes(fixture: &str, scratch: &Path) {
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
        "npm install failed for {fixture} node test at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    // Shell invocation so glob expansion picks up every test file —
    // passing the directory confuses tsx/node:test when there's no
    // index entry. tsx registers a module loader so `.ts` imports
    // resolve at run time.
    let output = Command::new("sh")
        .arg("-c")
        .arg("./node_modules/.bin/tsx --test test/*.test.ts")
        .current_dir(scratch)
        .output()
        .expect("run node --test via tsx");

    assert!(
        output.status.success(),
        "node --test failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
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
    assert_node_test_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_libsql_node_test_passes() {
    // libsql (`node-async`) profile: same forcing function as
    // `real_blog_node_test_passes` but exercises the async-coloring
    // emit path AND the libsql adapter at runtime. Every test
    // method awaits its HTTP-style helpers (`this.get(...)` etc.)
    // and AR calls (`Article.find(...)`); fixtures load through
    // `await _fixtures_load_bang()` which executes inserts on a
    // libsql in-memory client. Catches regressions in (a) the
    // runtime↔app propagation handshake, (b) the test-runtime
    // async surface (minitest.ts get/post/dispatch), and (c) the
    // libsql adapter's actual SQL execution.
    use roundhouse::profile::DeploymentProfile;

    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog-libsql-node");
    generate_project_with_profile(fixture, &scratch, &DeploymentProfile::node_async());
    assert_node_test_passes("real-blog (libsql)", &scratch);
}
