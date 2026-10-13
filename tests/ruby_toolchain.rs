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
//! `bundle exec rake test` against the emitted tree. The Rakefile
//! discovers `test/**/*_test.rb`; a hand-maintained list here is how
//! `views/articles_test.rb`, `views/articles_json_test.rb`, and
//! `models/article_broadcasts_test.rb` shipped for months without
//! ever running.

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

/// Every `*_test.rb` under `test/`, relative to the project root, sorted.
/// The same set `Rake::TestTask`'s `FileList["test/**/*_test.rb"]` names.
fn discover_test_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("readdir {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_test.rb"))
            {
                let rel = path
                    .strip_prefix(root)
                    .expect("test file under scratch")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("test"), root, &mut out);
    out.sort();
    out
}

#[test]
#[ignore]
fn real_blog_spinel_tests_pass() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    // Absolute path to the scaffold's Gemfile so BUNDLE_GEMFILE works
    // regardless of the spawned process's cwd.
    let gemfile = std::fs::canonicalize("runtime/spinel/scaffold/Gemfile")
        .expect("canonicalize scaffold Gemfile");

    let discovered = discover_test_files(&scratch);
    assert!(
        discovered.len() >= 23,
        "expected the emitted suite plus runtime/spinel/test, got {} files:\n{}",
        discovered.len(),
        discovered.join("\n"),
    );
    for required in [
        "test/views/articles_test.rb",
        "test/views/articles_json_test.rb",
        "test/models/article_broadcasts_test.rb",
        "test/route_helpers_test.rb",
        "test/tools/check_spinel_subset_test.rb",
    ] {
        assert!(
            discovered.iter().any(|p| p == required),
            "{required} is not in the emitted test tree:\n{}",
            discovered.join("\n"),
        );
    }

    let rakefile = std::fs::read_to_string(scratch.join("Rakefile")).expect("Rakefile");
    assert!(
        rakefile.contains("t.test_files = FileList[\"test/**/*_test.rb\"]"),
        "Rakefile no longer discovers every test file:\n{rakefile}",
    );

    // The runner's list, not ours. A narrowed FileList fails here even
    // when the walk above still sees the files on disk.
    let listed = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .arg("exec")
        .arg("ruby")
        .arg("-e")
        .arg("require \"rake\"; puts FileList[\"test/**/*_test.rb\"].sort")
        .current_dir(&scratch)
        .output()
        .expect("list rake test files");
    assert!(
        listed.status.success(),
        "rake file list failed\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&listed.stdout),
        String::from_utf8_lossy(&listed.stderr),
    );
    let mut rake_files: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    rake_files.sort();
    assert_eq!(
        rake_files, discovered,
        "rake's FileList drifted from the files on disk",
    );

    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .arg("exec")
        .arg("rake")
        .arg("test")
        .current_dir(&scratch)
        .output()
        .expect("spawn rake test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "rake test failed\n\
         \n=== stdout ===\n{stdout}\n\
         \n=== stderr ===\n{stderr}",
    );
    // Word-boundary: `"10 failures".contains("0 failures")` is true.
    assert!(
        stdout.contains(" 0 failures") && stdout.contains(" 0 errors"),
        "rake test exited 0 without a clean minitest summary:\n{stdout}\n{stderr}",
    );
}
