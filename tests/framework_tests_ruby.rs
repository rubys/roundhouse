//! Framework-test transpile gate (Ruby target, CRuby/MRI runtime).
//!
//! Mirrors `framework_tests_typescript.rs` for the Ruby target. The
//! gate uses stock CRuby as the runtime: the emit function (still
//! named `emit_spinel` for historical reasons) produces Ruby-shape
//! output that runs verbatim under MRI. The future Spinel-AOT
//! framework-tests job will run the same emit through the spinel
//! binary when end-to-end runnable. Same pattern `toolchain-ruby`
//! already uses for real-blog.
//!
//! What this catches that `framework_ruby_tests_pass` doesn't:
//! transpile-fidelity gaps in the test-file lowering itself
//! (`lower_test_modules_to_library_classes` rewrites of `test "..."`
//! macros, fixture references, etc.) and adapter-contract drift the
//! framework-Ruby gate can't see because it runs the source
//! verbatim.
//!
//! Marked `#[ignore]` while gaps close — run explicitly:
//!
//!     cargo test --test framework_tests_ruby -- --ignored --nocapture

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
    base.join("roundhouse-framework-tests-ruby").join(tag)
}

/// Recursively copy a tree. Used to seed the scratch dir with
/// framework runtime Ruby files at a flat layout the framework
/// test_helper expects (`$LOAD_PATH.unshift(FRAMEWORK_RUBY)` where
/// `FRAMEWORK_RUBY = File.expand_path("..", __dir__)` from
/// `<scratch>/test/test_helper.rb`).
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

fn build_and_run(test_file: &Path, tag: &str) {
    let scratch = scratch_dir(tag);
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(&scratch).expect("create scratch");

    // Framework runtime: mirror the production spinel layout
    // (`runtime/<name>.rb` under the project root) so the spinel
    // emit's require resolution — which produces paths like
    // `runtime/inflector` for cross-tree refs — works without
    // adjustment. Earlier flat layout (everything under scratch
    // root) didn't match what the emitter expects.
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
    ] {
        let src = runtime_ruby.join(entry);
        if src.exists() {
            std::fs::copy(&src, scratch_runtime.join(entry))
                .unwrap_or_else(|_| panic!("copy {entry}"));
        }
    }

    // Test helper: same framework-Ruby version the source-side
    // `framework_ruby_tests_pass` gate uses, but with FRAMEWORK_RUBY
    // re-pointed at `<scratch>/runtime/` (the spinel layout). The
    // source-side helper sets `FRAMEWORK_RUBY = File.expand_path("..",
    // __dir__)` (i.e. parent of `test/`), which under the source
    // tree is `runtime/ruby/` — exactly where the framework files
    // live. Under our scratch layout the files live at `<scratch>/
    // runtime/` (mirroring the production transpiled-blog path),
    // so adjust the expand_path call before writing.
    std::fs::create_dir_all(scratch.join("test")).expect("mkdir test");
    let helper_src = std::fs::read_to_string(runtime_ruby.join("test/test_helper.rb"))
        .expect("read framework test_helper");
    let helper_patched = helper_src.replace(
        r#"FRAMEWORK_RUBY = File.expand_path("..", __dir__)"#,
        r#"FRAMEWORK_RUBY = File.expand_path("../runtime", __dir__)"#,
    );
    std::fs::write(scratch.join("test/test_helper.rb"), helper_patched)
        .expect("write patched test_helper");

    // Ingest the single framework test file as a TestModule, drop it
    // onto an otherwise-empty App, run analyze + spinel emit. emit_spinel
    // lowers test-class shape (test "..." → def test_..., fixture refs,
    // parent-class swap), writes the test file under
    // test/{models,controllers}/<stem>_test.rb. Empty App means no
    // schema / routes / models — just the test file plus a default
    // (real-blog-shaped) test_helper that we'll overwrite below.
    let source = std::fs::read(test_file)
        .unwrap_or_else(|e| panic!("read {}: {e}", test_file.display()));
    let test_module = ingest_test_file(&source, &test_file.display().to_string())
        .expect("ingest framework test file")
        .expect("framework test file should contain a test class");

    let mut app = App::new();
    app.test_modules.push(test_module);
    Analyzer::new(&app).analyze(&mut app);

    for file in ruby::emit_spinel(&app) {
        // Skip the spinel-emitted test_helper.rb — we already wrote
        // the framework-Ruby version above. The spinel one expects
        // sqlite + schema + fixtures (real-blog-shaped); framework
        // tests need only the FrameworkTestAdapter shim.
        if file.path == PathBuf::from("test/test_helper.rb") {
            continue;
        }
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir emit parent");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    // Locate the emitted test file. emit_spinel writes it under
    // test/models/<stem>_test.rb or test/controllers/<stem>_test.rb
    // depending on the class-name suffix. Framework tests don't
    // follow the *ControllerTest naming convention, so they'll land
    // under test/models/ regardless. Find whichever file got written.
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

    // Run via bundle exec ruby. Reuse the spinel scaffold's Gemfile
    // (it has minitest + sqlite3 + rake; only minitest is actually
    // required by these tests, but bundler resolves the lock once).
    let gemfile = std::fs::canonicalize("runtime/spinel/scaffold/Gemfile")
        .expect("canonicalize scaffold Gemfile");

    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .arg("exec")
        .arg("ruby")
        .arg("-Itest")
        .arg("-I.")
        .arg(&test_rel)
        .current_dir(&scratch)
        .output()
        .expect("spawn ruby");

    assert!(
        output.status.success(),
        "framework test failed: {} (emitted to {})\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        emitted_test.path().display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn validations_test_passes_under_cruby() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/validations_test.rb"),
        "validations",
    );
}

#[test]
#[ignore]
fn ar_base_test_passes_under_cruby() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/base_test.rb"),
        "ar_base",
    );
}

#[test]
#[ignore]
fn errors_test_passes_under_cruby() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/errors_test.rb"),
        "errors",
    );
}

#[test]
#[ignore]
fn parameters_test_passes_under_cruby() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/parameters_test.rb"),
        "parameters",
    );
}

#[test]
#[ignore]
fn ac_base_test_passes_under_cruby() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/base_test.rb"),
        "ac_base",
    );
}

#[test]
#[ignore]
fn router_test_passes_under_cruby() {
    build_and_run(
        Path::new("runtime/ruby/test/action_dispatch/router_test.rb"),
        "router",
    );
}

#[test]
#[ignore]
fn view_helpers_test_passes_under_cruby() {
    build_and_run(
        Path::new("runtime/ruby/test/action_view/view_helpers_test.rb"),
        "view_helpers",
    );
}

#[test]
#[ignore]
fn inflector_test_passes_under_cruby() {
    build_and_run(
        Path::new("runtime/ruby/test/inflector_test.rb"),
        "inflector",
    );
}
