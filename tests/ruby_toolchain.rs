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
use roundhouse::ingest::ingest_app;
use roundhouse::project::BuildTarget;

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

/// Build the scratch project: the REAL ruby-target file set, exactly as
/// `roundhouse --target ruby` ships it.
///
/// This used to hand-copy a curated list of runtime files on top of the
/// scaffold — a third copy of "what the ruby target needs", alongside
/// `project::ruby_runtime_files` (the one that actually ships) and
/// `spinel_toolchain.rs`. It drifted, silently, in the direction that
/// matters least until something depends on it: the harness passed while
/// the tree it built was missing 20-odd runtime files the shipped tree
/// has. The moment `test/test_helper.rb` started requiring `main.rb`
/// (whose chain is complete), the gap surfaced as
/// `cannot load such file -- runtime/active_support_duration`.
///
/// A toolchain test should exercise what ships. `target_files` IS what
/// ships, so ask it.
fn generate_project(fixture: &Path, scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(scratch).expect("create scratch");

    let mut app = ingest_app(fixture).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = roundhouse::project::target_files(&app, fixture, BuildTarget::Ruby)
        .expect("ruby target files");
    roundhouse::project::write_to_dir(&files, scratch).expect("write ruby target tree");

    // The framework runtime's OWN tests (broadcasts/cgi_io + the
    // integration/views/models/tools subdirs) are a harness concern, not
    // something an app archive ships — overlay them so this job keeps
    // covering them alongside the app's emitted suite.
    copy_tree(Path::new("runtime/spinel/test"), &scratch.join("test"));

    // …but not that tree's `test_helper.rb`: the shipped tree already
    // carries the per-app rendered one (`render_test_helper`), and the
    // source copy is the blog-shaped stand-in it exists to replace.
    for file in &files {
        if file.0 == "test/test_helper.rb" {
            std::fs::write(scratch.join("test/test_helper.rb"), &file.1)
                .expect("restore rendered test_helper");
        }
    }
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
        // `has_json`'s column seam (`runtime/schematized_json.rb`) — a
        // per-target runtime module, so this is the lane that runs its
        // unit tests. Rides in the same way query_count_test does.
        "test/schematized_json_test.rb",
        // `runtime/broadcasts.rb` plus the two CAPTURE helpers in
        // test_helper.rb, which are not the same shape as each other
        // (`assert_turbo_stream_broadcasts` is cumulative at the pinned
        // turbo-rails, `assert_broadcasts` is a delta) and had no gate
        // at all: this file shipped in every ruby emit and nothing ever
        // ran it. Rides in the same way the two above do.
        "test/broadcasts_test.rb",
        // The `Dom` selector stub in test_helper.rb — `assert_select`'s
        // whole substrate, and string rules all the way down, so a
        // silently-wrong one makes an assertion UNPASSABLE rather than
        // loose (which reads as a missing feature in the app under
        // test, not as a broken matcher). Rides in the same way the
        // three above do.
        "test/dom_test.rb",
        // A TIME bind and a temporal COLUMN must be written in the same
        // format — the adapter inlines escaped values rather than
        // binding parameters, so a comparison is string-vs-string and a
        // mismatched bind answers the wrong rows with no error at all.
        // Rides in the same way the others do.
        "test/temporal_bind_test.rb",
        // `Relation#find` raising `RecordNotFound` (Rails' whole
        // distinction between it and `find_by`) and the harness's
        // `parsed_body`. Both are RUNTIME behavior the compiler's own
        // suite cannot see, and both fail quietly — a nil `find` is a
        // NoMethodError several frames from the lookup. Rides in the
        // same way the others do.
        "test/relation_find_test.rb",
    ] {
        assert_test_passes(&scratch, &gemfile, test);
    }
}
