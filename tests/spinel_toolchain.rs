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
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app;

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-spinel-{tag}"))
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

fn generate_project(fixture: &Path, scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(scratch).expect("create scratch");

    let scaffold = Path::new("runtime/spinel/scaffold");
    copy_tree(scaffold, scratch);

    copy_tree(Path::new("runtime/spinel/test"), &scratch.join("test"));

    let runtime_ruby = Path::new("runtime/ruby");
    for entry in [
        "active_record",
        "action_view",
        "action_controller",
        "action_dispatch",
    ] {
        let src = runtime_ruby.join(entry);
        if src.exists() {
            copy_tree(&src, &scratch.join("runtime").join(entry));
        }
    }
    for entry in [
        "active_record.rb",
        "action_view.rb",
        "action_controller.rb",
        "action_dispatch.rb",
        "inflector.rb",
        "json_builder.rb",
    ] {
        std::fs::copy(
            runtime_ruby.join(entry),
            scratch.join("runtime").join(entry),
        )
        .unwrap_or_else(|_| panic!("copy {entry}"));
    }
    let runtime_spinel = Path::new("runtime/spinel");
    for entry in [
        "sqlite_adapter.rb",
        "cgi_io.rb",
        "broadcasts.rb",
        "base64.rb",
        "json.rb",
        "importmap.rb",
    ] {
        std::fs::copy(
            runtime_spinel.join(entry),
            scratch.join("runtime").join(entry),
        )
        .unwrap_or_else(|_| panic!("copy {entry}"));
    }
    // FFI variant for the spinel target — test_helper.rb requires
    // `../runtime/db`, which resolves to whichever sibling we drop in.
    // The Ruby toolchain test drops `db_cruby.rb` instead.
    std::fs::copy(
        runtime_spinel.join("db.rb"),
        scratch.join("runtime").join("db.rb"),
    )
    .expect("copy db.rb -> runtime/db.rb");

    let mut app = ingest_app(fixture).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    for file in ruby::emit_spinel(&app) {
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir emit parent");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
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
