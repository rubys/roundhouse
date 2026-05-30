//! Ruby toolchain integration test — runs CRuby/MRI over the emitted
//! Ruby-shape output (app + tests) to assert the lowering produces a
//! project that satisfies real-blog's test contract.
//!
//! Symmetry with other toolchain jobs: TypeScript / Rust / Crystal /
//! etc. emit both the app AND its tests, then run the emitted tests
//! against the emitted app. The Ruby target does the same — the
//! emit function (still named `emit_spinel` for historical reasons)
//! produces `test/test_helper.rb`, `test/fixtures/<plural>.rb`, and
//! `test/{models,controllers}/<stem>_test.rb` from real-blog's test
//! sources. The future Spinel-AOT toolchain job will run the same
//! emit through the spinel binary when end-to-end runnable.
//!
//! Marked `#[ignore]` so it doesn't run in the default `cargo test`
//! sweep — the bundle install + Ruby invocation costs are CI-only.
//! Run explicitly:
//!
//!     cargo test --test ruby_toolchain -- --ignored --nocapture
//!
//! Layout: emit lowered files into a scratch dir overlaid on a copy of
//! `runtime/spinel/scaffold/` (Gemfile, inner Makefile, main.rb,
//! app/views.rb — a hand-written aggregator we don't yet emit, tools/),
//! `runtime/spinel/test/` (target-specific tests), plus the framework
//! Ruby + per-target primitives flattened into `runtime/`. Then
//! `bundle exec ruby` each model/controller test against the emitted
//! code.
//!
//! Suites validated: article + comment model tests, articles + comments
//! controller tests. article_broadcasts and the views suite have known
//! gaps tracked in `project_lowered_ir_gaps_for_runnability` and aren't
//! gating yet.

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app;

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-ruby-{tag}"))
}

/// Recursively copy a tree. Used to seed the scratch dir with
/// runtime/spinel scaffolding before overlaying emitted files.
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

/// Build the scratch project: runtime/spinel scaffold + emitted spinel
/// app/. Returns the scratch path.
fn generate_project(fixture: &Path, scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(scratch).expect("create scratch");

    // Verbatim scaffold (Gemfile, inner Makefile, main.rb, app/views.rb,
    // app/assets/tailwind.css, server/, tools/, .gitignore). Bundler
    // resolves against the scratch's own Gemfile via BUNDLE_GEMFILE in
    // assert_test_passes.
    let scaffold = Path::new("runtime/spinel/scaffold");
    copy_tree(scaffold, scratch);

    // Target-specific tests (broadcasts/cgi_io at the top level +
    // integration/views/models/tools subdirs).
    copy_tree(Path::new("runtime/spinel/test"), &scratch.join("test"));

    // Runtime: framework Ruby + spinel target primitives, both flat
    // under scratch/runtime/. The scratch simulates the eventual
    // Spinel-target layout where runtime/ is a flat tree of framework
    // code (ruby) + primitive runtime (spinel).
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
    // Db primitive surface — the Ruby target gets the gem-backed
    // variant (`db_cruby.rb` source) materialized as `db.rb` in the
    // emitted tree so main.rb's `require_relative "runtime/db"`
    // resolves to the CRuby-runnable shim. The FFI variant
    // (`runtime/spinel/db.rb`) is reserved for a future Spinel-AOT
    // target's tree; not shipped to the Ruby target.
    std::fs::copy(
        runtime_spinel.join("db_cruby.rb"),
        scratch.join("runtime").join("db.rb"),
    )
    .expect("copy db_cruby.rb -> runtime/db.rb");
    // Emit the spinel-shape app/ from real-blog and write into scratch.
    // emit_spinel writes its own `test/test_helper.rb` from the canonical
    // at `runtime/spinel/test/`, overwriting the copy laid down above
    // (same content; harmless).
    let mut app = ingest_app(fixture).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    for file in ruby::emit_spinel(&app) {
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir emit parent");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    // emit_spinel writes test/{models,controllers}/*_test.rb in
    // real-blog's shape and the spinel runtime now supports the
    // Rails-idiom surface those tests use (fixture persistence via
    // FixtureLoader, assert_response/assert_select/
    // assert_no_difference shims, ActionDispatch::IntegrationTest
    // parent class, single-arg assert_redirected_to). No overlay —
    // the emitted tests run as-is.
}

/// Run a single test file via `bundle exec ruby -Itest -I.` and assert
/// it exits zero. Bundler resolves against
/// `runtime/spinel/scaffold/Gemfile` (set via BUNDLE_GEMFILE) so the
/// gem cache populated by CI's ruby/setup-ruby step is reused.
fn assert_test_passes(scratch: &Path, gemfile: &Path, test_path: &str) {
    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", gemfile)
        .arg("exec")
        .arg("ruby")
        .arg("-Itest")
        .arg("-I.")
        .arg(test_path)
        .current_dir(scratch)
        .output()
        .expect("spawn ruby");

    assert!(
        output.status.success(),
        "spinel test failed: {test_path}\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_spinel_tests_pass() {
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    // Absolute path to the scaffold's Gemfile so BUNDLE_GEMFILE works
    // regardless of the spawned process's cwd.
    let gemfile = std::fs::canonicalize("runtime/spinel/scaffold/Gemfile")
        .expect("canonicalize scaffold Gemfile");

    for test in [
        "test/models/article_test.rb",
        "test/models/comment_test.rb",
        "test/controllers/articles_controller_test.rb",
        "test/controllers/comments_controller_test.rb",
        // Query-count harness (issue #27): asserts /articles eager-loads
        // comments in 2 queries rather than the 1+N `compare` is blind
        // to. Rides in via runtime/spinel/test/query_count_test.rb.
        "test/query_count_test.rb",
    ] {
        assert_test_passes(&scratch, &gemfile, test);
    }
}
