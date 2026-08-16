//! Spinel toolchain integration test — compiles the emitted real-blog
//! tests via the spinel AOT compiler and runs the resulting native
//! binaries. Mirrors `ruby_toolchain.rs`: same emit, same 4 test
//! suites, swapped runner.
//!
//! Two differences from the Ruby toolchain test:
//!   1. `runtime/db.rb` is the FFI-backed shim (`runtime/spinel/db.rb`,
//!      module Db over libsqlite3) rather than the gem-backed sibling.
//!   2. The runner is the scaffold Makefile's `spinel-test` target,
//!      which compiles each `test/<dir>/<stem>.rb` via `$(SPINEL)` and
//!      executes the resulting binary. `$(SPINEL)` defaults to `spinel`
//!      on PATH — set the `SPINEL` env var to override.
//!
//! Marked `#[ignore]` — CI-only. Invoke:
//!
//!     cargo test --test spinel_toolchain -- --ignored --nocapture
//!
//! Prerequisites for local runs: `spinel` on PATH (or `SPINEL=...`),
//! and `libsqlite3.so` discoverable at link time (`libsqlite3-dev` on
//! Debian/Ubuntu; macOS ships it).
//!
//! Suites validated: same 4 as ruby_toolchain — article + comment
//! model tests, articles + comments controller tests. Wider coverage
//! (article_broadcasts, views suite) tracked in
//! `project_lowered_ir_gaps_for_runnability`.

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;

use roundhouse::ingest::ingest_app;

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-spinel-{tag}"))
}

/// Move every `<scratch>/{runtime,test}/**/*.rbs` to
/// `<scratch>/sig/{runtime,test}/<rel>.rbs`. Mirrors the top-level
/// Makefile's RUBY_OUT layout (f6d2b87): one `sig/` root for both
/// hand-authored RBS (runtime tree + test_helper) and roundhouse-emitted
/// app RBS.
fn reroute_runtime_rbs_to_sig(scratch: &Path) {
    fn walk(dir: &Path, src_root: &Path, sig_root: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return; };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, src_root, sig_root);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rbs") {
                let rel = path.strip_prefix(src_root).expect("under src root");
                let dst = sig_root.join(rel);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).expect("mkdir sig parent");
                }
                std::fs::rename(&path, &dst).expect("mv .rbs to sig/");
            }
        }
    }
    let runtime_dir = scratch.join("runtime");
    let sig_runtime = scratch.join("sig").join("runtime");
    walk(&runtime_dir, &runtime_dir, &sig_runtime);

    let test_dir = scratch.join("test");
    let sig_test = scratch.join("sig").join("test");
    walk(&test_dir, &test_dir, &sig_test);
}

fn copy_tree(src: &Path, dst: &Path) {
    if src.is_dir() {
        std::fs::create_dir_all(dst).expect("mkdir");
        for entry in std::fs::read_dir(src).expect("readdir") {
            let entry = entry.expect("entry");
            copy_tree(&entry.path(), &dst.join(entry.file_name()));
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::copy(src, dst).expect("copy file");
    }
}

/// Build the scratch project: the REAL spinel base file set, exactly as
/// `project::spinel_files` assembles it before `spin_shape`.
///
/// This used to hand-copy an enumerated list of runtime files on top of
/// the scaffold. Its own comment called that "a FOURTH registration
/// point for a new runtime file" and predicted the failure mode — "a
/// miss shows up as `cannot load such file` from spinel rather than
/// from anything the unit tests reach" — which is exactly how it broke
/// once `test/test_helper.rb` started requiring `main.rb` (whose chain
/// is complete). Deleted in favour of the set that ships.
fn generate_project(fixture: &Path, scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(scratch).expect("create scratch");

    let mut app = ingest_app(fixture).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, fixture).expect("spinel base files");
    roundhouse::project::write_to_dir(&files, scratch).expect("write spinel tree");

    // The framework runtime's OWN tests (broadcasts/cgi_io + the
    // integration/views/models/tools subdirs) are a harness concern, not
    // something an app archive ships — overlay them so this job keeps
    // covering them alongside the app's emitted suite.
    copy_tree(Path::new("runtime/spinel/test"), &scratch.join("test"));

    // …but not that tree's `test_helper.rb`: the shipped tree already
    // carries the per-app rendered one (`render_test_helper`), and the
    // source copy is the blog-shaped stand-in it exists to replace.
    for (path, content) in &files {
        if path == "test/test_helper.rb" {
            std::fs::write(scratch.join("test/test_helper.rb"), content)
                .expect("restore rendered test_helper");
        }
    }
}

#[test]
#[ignore]
fn real_blog_spinel_tests_pass() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("make")
        .arg("spinel-test")
        .current_dir(&scratch)
        .output()
        .expect("spawn make spinel-test");

    assert!(
        output.status.success(),
        "make spinel-test failed\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
