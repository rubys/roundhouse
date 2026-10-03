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

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/class_configuration.rs"]
mod class_configuration;
#[path = "support/rails_root_join.rs"]
mod rails_root_join;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn finite_concern_class_configuration_runs_natively() {
    for (overlay, assertions) in [
        (class_configuration::overlay(), class_configuration::ASSERTIONS),
        (class_configuration::empty_overlay(), class_configuration::EMPTY_ASSERTIONS),
    ] {
        let run = overlay.run_spinel(assertions);
        run.assert_passes();
        assert!(run.stdout.contains("finite class configuration contract passed"));
    }
}

/// The native half of `emit_and_run::rails_root_join_takes_any_number_of_parts`:
/// the fixtures never call `join` with more than one part, so no other
/// Spinel lane compiles the variadic `Rails::AppPath#join`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn rails_root_join_takes_any_number_of_parts_natively() {
    let run = rails_root_join::overlay().run_spinel(rails_root_join::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("Rails.root.join contract passed"));
}

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
    let fixture = roundhouse::fixtures::real_blog();
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
