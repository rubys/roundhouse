//! Framework-test transpile gate (spinel AOT target).
//!
//! Mirrors `framework_tests_ruby.rs` but compiles the emitted test
//! through `spinel --rbs sig` and runs the resulting native binary
//! instead of stock CRuby. Catches spinel-specific divergences in the
//! framework-runtime layer (type-narrowing gaps, RBS application,
//! monomorphization edge cases) that the source-side `framework_tests_
//! ruby` gate can't surface because CRuby is dynamic.
//!
//! Marked `#[ignore]` (CI-only). Invoke:
//!
//!     PATH=$HOME/git/spinel:$PATH cargo test --test framework_tests_spinel -- --ignored --nocapture
//!
//! Status (2026-05-20): 3/6 tests pass under spinel; the CI job is
//! `continue-on-error: true` while the other three close.
//!   ✓ inflector  ✓ router  ✓ ac_base
//!   ✗ ar_base       — references FrameworkTestAdapter (Hash[Symbol|
//!                     String, untyped] doesn't survive spinel
//!                     monomorphization; minimal helper here omits it
//!                     so the missing-constant cascade is visible)
//!   ✗ errors        — `assert_operator <class>, :<, <class>` uses
//!                     Class-as-value via .send(op, ...); known cross-
//!                     target non-translatable per the comment in
//!                     runtime/ruby/test/test_helper.rb
//!   ⚠ view_helpers  — source defines `class Article < AR::Base` +
//!                     `class ViewHelpersTest`; emit_spinel routes
//!                     Article to test/models/ and drops the actual
//!                     test class. Harness reports false-positive
//!                     "0 tests passed" — roundhouse emit bug, not
//!                     spinel.

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_test_file;
use roundhouse::App;

fn scratch_dir(tag: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse-framework-tests-spinel").join(tag)
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

/// Move every `<scratch>/{runtime,test}/**/*.rbs` to
/// `<scratch>/sig/{runtime,test}/<rel>.rbs`. Same pattern as
/// `spinel_toolchain.rs::reroute_runtime_rbs_to_sig`.
fn reroute_rbs_to_sig(scratch: &Path) {
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

fn build_and_run(test_file: &Path, tag: &str) {
    let scratch = scratch_dir(tag);
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(&scratch).expect("create scratch");

    let runtime_ruby = Path::new("runtime/ruby");
    let scratch_runtime = scratch.join("runtime");
    std::fs::create_dir_all(&scratch_runtime).expect("mkdir runtime");
    for entry in [
        "active_record",
        "action_view",
        "action_controller",
        "action_dispatch",
    ] {
        let src = runtime_ruby.join(entry);
        if src.exists() {
            copy_tree(&src, &scratch_runtime.join(entry));
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
        let src = runtime_ruby.join(entry);
        if src.exists() {
            std::fs::copy(&src, scratch_runtime.join(entry))
                .unwrap_or_else(|_| panic!("copy {entry}"));
        }
    }

    std::fs::create_dir_all(scratch.join("test")).expect("mkdir test");
    // Minimal spinel-compatible test_helper. The framework-Ruby helper
    // at runtime/ruby/test/test_helper.rb defines an in-memory
    // `FrameworkTestAdapter` whose polymorphic Hash[Symbol|String,
    // untyped] shape doesn't survive spinel monomorphization (and
    // tanks all six framework tests at C compile time before any
    // test code even runs). For framework_tests_spinel we want each
    // test's failures to surface its OWN spinel-side gaps, not be
    // masked by the helper. So this helper just provides TestBase
    // and pulls in the framework runtime modules via require_relative
    // — the tests that need the adapter (ar_base, errors) will
    // surface their specific missing-adapter failure mode at runtime,
    // tests that don't (inflector, router, view_helpers) compile and
    // run clean against the framework runtime alone.
    let helper = r#"# Auto-generated by framework_tests_spinel.rs.
require_relative "../runtime/inflector"
require_relative "../runtime/active_record"
require_relative "../runtime/action_view/view_helpers"
require_relative "../runtime/action_dispatch/router"
require_relative "../runtime/action_controller/base"

class TestBase
  def initialize
  end

  def setup
  end

  def teardown
  end

  def assert_operator(lhs, op, rhs, msg = nil)
    return if lhs.send(op, rhs)
    raise(msg || "assert_operator failed")
  end

  def assert_match(pattern, value, msg = nil)
    raise(msg || "assert_match: nil value") if value.nil?
    return if value =~ pattern
    raise(msg || "assert_match failed")
  end
end
"#;
    std::fs::write(scratch.join("test/test_helper.rb"), helper)
        .expect("write minimal test_helper");

    let source = std::fs::read(test_file)
        .unwrap_or_else(|e| panic!("read {}: {e}", test_file.display()));
    let test_module = ingest_test_file(&source, &test_file.display().to_string())
        .expect("ingest framework test file")
        .expect("framework test file should contain a test class");

    let mut app = App::new();
    app.test_modules.push(test_module);
    Analyzer::new(&app).analyze(&mut app);

    for file in ruby::emit_spinel(&app) {
        if file.path == PathBuf::from("test/test_helper.rb") {
            continue;
        }
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir emit parent");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    // After the emit, reroute .rbs sidecars into sig/. spinel's
    // `--rbs sig` flag walks sig/, not the file-adjacent layout.
    reroute_rbs_to_sig(&scratch);

    // Locate the emitted test file (same logic as the ruby variant).
    let test_dir = scratch.join("test/models");
    let emitted_test = std::fs::read_dir(&test_dir)
        .unwrap_or_else(|e| panic!("readdir {}: {e}", test_dir.display()))
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().is_some_and(|x| x == "rb"))
        .expect("find emitted test file");
    let test_rel = emitted_test
        .path()
        .strip_prefix(&scratch)
        .expect("emitted path under scratch")
        .to_string_lossy()
        .into_owned();
    let stem = emitted_test
        .path()
        .file_stem()
        .expect("file stem")
        .to_string_lossy()
        .into_owned();
    let bin_path = format!("build/{stem}");

    std::fs::create_dir_all(scratch.join("build")).expect("mkdir build");

    // Compile with spinel.
    let compile = Command::new("spinel")
        .arg("--rbs")
        .arg("sig")
        .arg(&test_rel)
        .arg("-o")
        .arg(&bin_path)
        .current_dir(&scratch)
        .output()
        .expect("spawn spinel");

    assert!(
        compile.status.success(),
        "spinel compile failed: {} (emitted to {})\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        emitted_test.path().display(),
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    // Execute the resulting binary.
    let output = Command::new(format!("./{bin_path}"))
        .current_dir(&scratch)
        .output()
        .expect("spawn test binary");

    assert!(
        output.status.success(),
        "framework test failed: {} (binary {})\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        bin_path,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn ar_base_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/base_test.rb"),
        "ar_base",
    );
}

#[test]
#[ignore]
fn errors_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/errors_test.rb"),
        "errors",
    );
}

#[test]
#[ignore]
fn ac_base_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/base_test.rb"),
        "ac_base",
    );
}

#[test]
#[ignore]
fn router_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_dispatch/router_test.rb"),
        "router",
    );
}

#[test]
#[ignore]
fn view_helpers_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_view/view_helpers_test.rb"),
        "view_helpers",
    );
}

#[test]
#[ignore]
fn inflector_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/inflector_test.rb"),
        "inflector",
    );
}
