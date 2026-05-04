//! Spinel toolchain integration test — runs CRuby over the emitted
//! spinel-shape output (app + tests) to assert the lowering produces a
//! project that satisfies real-blog's test contract.
//!
//! Symmetry with other toolchain jobs: TypeScript / Rust / Crystal /
//! etc. emit both the app AND its tests, then run the emitted tests
//! against the emitted app. Spinel does the same — `emit_spinel`
//! emits `test/test_helper.rb`, `test/fixtures/<plural>.rb`, and
//! `test/{models,controllers}/<stem>_test.rb` from real-blog's test
//! sources. CRuby is the runtime (spinel is a Ruby subset; spinel's
//! own test runner can swap in once it lands).
//!
//! Marked `#[ignore]` so it doesn't run in the default `cargo test`
//! sweep — the bundle install + Ruby invocation costs are CI-only.
//! Run explicitly:
//!
//!     cargo test --test spinel_toolchain -- --ignored --nocapture
//!
//! Layout: emit lowered files into a scratch dir overlaid on a copy of
//! `fixtures/spinel-blog/{runtime,test,config,Gemfile,Gemfile.lock,
//! main.rb,Rakefile,server,tools}` (plus `app/views.rb` — a hand-written
//! aggregator we don't yet emit), then `bundle exec ruby` each
//! model/controller test against the emitted code.
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
    std::env::temp_dir().join(format!("roundhouse-spinel-{tag}"))
}

/// Recursively copy a tree. Used to seed the scratch dir with
/// spinel-blog scaffolding before overlaying emitted files.
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

/// Build the scratch project: spinel-blog scaffold + emitted spinel
/// app/. Returns the scratch path.
fn generate_project(fixture: &Path, scaffold: &Path, scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(scratch).expect("create scratch");

    // Scaffolding from spinel-blog. `app/views.rb` is the hand-written
    // aggregator (`module Views; require_relative ... end`); the rest
    // are the runtime, test suite, etc. Gemfile is NOT copied — we
    // point bundler at spinel-blog's via BUNDLE_GEMFILE so the gem
    // cache from ruby/setup-ruby (which runs `bundle install` against
    // fixtures/spinel-blog) is reused.
    for entry in ["runtime", "test", "config", "server", "tools", "main.rb", "Rakefile"] {
        let src = scaffold.join(entry);
        if src.exists() {
            copy_tree(&src, &scratch.join(entry));
        }
    }
    std::fs::create_dir_all(scratch.join("app")).expect("mkdir app");
    std::fs::copy(
        scaffold.join("app/views.rb"),
        scratch.join("app/views.rb"),
    )
    .expect("copy app/views.rb");

    // Replace the bridge `.rb` files in scratch/runtime/ with the
    // canonical files from runtime/{ruby,spinel}/. The bridges in
    // fixtures/spinel-blog/runtime/ route to ../../../runtime/{ruby,spinel}/
    // — which doesn't resolve from the scratch dir. The scratch
    // simulates the eventual Spinel-target layout where runtime/ is
    // a flat tree of framework code (ruby) + primitive runtime (spinel).
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
        "in_memory_adapter.rb",
        "cgi_io.rb",
        "broadcasts.rb",
    ] {
        std::fs::copy(
            runtime_spinel.join(entry),
            scratch.join("runtime").join(entry),
        )
        .unwrap_or_else(|_| panic!("copy {entry}"));
    }
    // Emit the spinel-shape app/ from real-blog and write into scratch.
    // emit_spinel writes its own `test/test_helper.rb` from the canonical
    // at `runtime/spinel/test/`, overwriting the bridge that was copied
    // in by the spinel-blog scaffold above.
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
/// it exits zero. Bundler resolves against `fixtures/spinel-blog/Gemfile`
/// (set via BUNDLE_GEMFILE) so the gem cache populated by CI's
/// ruby/setup-ruby step is reused.
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
    let scaffold = Path::new("fixtures/spinel-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, scaffold, &scratch);

    // Absolute path to spinel-blog's Gemfile so BUNDLE_GEMFILE works
    // regardless of the spawned process's cwd.
    let gemfile = std::fs::canonicalize(scaffold.join("Gemfile"))
        .expect("canonicalize spinel-blog Gemfile");

    for test in [
        "test/models/article_test.rb",
        "test/models/comment_test.rb",
        "test/controllers/articles_controller_test.rb",
        "test/controllers/comments_controller_test.rb",
    ] {
        assert_test_passes(&scratch, &gemfile, test);
    }
}
