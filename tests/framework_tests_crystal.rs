//! Framework-test transpile gate (Crystal target).
//!
//! Mirrors `framework_tests_typescript.rs` and `framework_tests_spinel.rs`
//! — ingests one `runtime/ruby/test/**/*_test.rb` file as a TestModule,
//! drops it onto an otherwise-empty App, runs `crystal::emit`, and
//! invokes `crystal spec` against the result.
//!
//! What this catches that `framework_ruby_tests_pass` doesn't:
//! transpile-fidelity gaps in the Ruby→Crystal lowering of the test
//! file itself (test-class shape, fixture refs, parent-class swap to
//! `RoundhouseTest`) and Crystal-runtime adapter-contract drift.
//!
//! Marked `#[ignore]` while gaps close — run explicitly:
//!
//!     cargo test --test framework_tests_crystal -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::crystal;
use roundhouse::ingest::ingest_test_file;
use roundhouse::App;

/// Walk `runtime/ruby/**/*.rbs` and merge each parsed signature into
/// `app.rbs_signatures`. Without this the test body-typer can't
/// dispatch precisely against framework methods, and the strict-
/// typed Crystal emit falls through to the default `Ty::Untyped →
/// String` collapse. Same helper as `framework_tests_typescript`
/// (intentional duplication — keeping each gate self-contained).
fn load_framework_rbs(app: &mut App) {
    let runtime_ruby = Path::new("runtime/ruby");
    fn walk(dir: &Path, app: &mut App) {
        let Ok(entries) = std::fs::read_dir(dir) else { return; };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, app);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("rbs") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else { continue; };
            let Ok(sigs) = roundhouse::rbs::parse_app_signatures(&source) else { continue; };
            for (class_id, methods) in sigs {
                app.rbs_signatures.entry(class_id).or_default().extend(methods);
            }
        }
    }
    walk(runtime_ruby, app);
}

fn scratch_dir(tag: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse-framework-tests-crystal").join(tag)
}

fn build_and_run(test_file: &Path, tag: &str) {
    let scratch = scratch_dir(tag);
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(&scratch).expect("create scratch");

    let source = std::fs::read(test_file)
        .unwrap_or_else(|e| panic!("read {}: {e}", test_file.display()));
    let test_module = ingest_test_file(&source, &test_file.display().to_string())
        .expect("ingest framework test file")
        .expect("framework test file should contain a test class");

    let mut app = App::new();
    app.test_modules.push(test_module);
    load_framework_rbs(&mut app);
    Analyzer::new(&app).analyze(&mut app);

    for file in crystal::emit(&app) {
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    // `shards install` for sqlite3 / DB::Any — required by db.cr in
    // the emitted runtime even when the test file itself doesn't
    // touch the database. Per-test cache so parallel installs don't
    // race on the global ~/.cache/shards index.
    let install = Command::new("shards")
        .arg("install")
        .current_dir(&scratch)
        .env("SHARDS_CACHE_PATH", scratch.join(".shards-cache"))
        .output()
        .expect("run shards install");
    assert!(
        install.status.success(),
        "shards install failed at {}:\n=== stdout ===\n{}\n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    let output = Command::new("crystal")
        .arg("spec")
        .current_dir(&scratch)
        .env("CRYSTAL_CACHE_DIR", scratch.join(".crystal-cache"))
        .output()
        .expect("run crystal spec");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "crystal spec failed for {} at {}:\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        scratch.display(),
        stdout,
        stderr,
    );

    assert_tests_ran(&stdout, test_file, &scratch);
}

/// Defense against issue #4: `crystal spec` exits 0 when the
/// emitted spec file registers no `it`/`describe` blocks. When
/// emit-routing drops the `*Test` class (e.g. the source file also
/// defines a `< AR::Base` helper class that gets picked up first),
/// the resulting `.cr` carries the helper but no test definitions
/// and Crystal reports `0 examples` while passing. Parse the spec
/// summary line and require at least one example ran.
fn assert_tests_ran(stdout: &str, test_file: &Path, scratch: &Path) {
    let count = stdout.lines().find_map(parse_crystal_examples).unwrap_or_else(|| {
        panic!(
            "framework test for {} produced no crystal spec summary line — \
             cannot verify tests actually ran (see issue #4).\n\
             scratch: {}\n=== stdout ===\n{}",
            test_file.display(),
            scratch.display(),
            stdout,
        )
    });
    assert!(
        count >= 1,
        "framework test for {} reported 0 examples ran — \
         emit-routing likely dropped the test class (see issue #4).\n\
         scratch: {}\n=== stdout ===\n{}",
        test_file.display(),
        scratch.display(),
        stdout,
    );
}

/// Crystal spec summary: `5 examples, 0 failures, 0 errors, 0 pending`.
fn parse_crystal_examples(line: &str) -> Option<usize> {
    let line = line.trim();
    let idx = line.find(" examples, ")?;
    line[..idx].split_whitespace().last()?.parse::<usize>().ok()
}

#[test]
#[ignore]
fn inflector_test_passes_under_crystal() {
    build_and_run(
        Path::new("runtime/ruby/test/inflector_test.rb"),
        "inflector",
    );
}

// ar_base_test_passes_under_crystal — disabled. base_test.rb depends
// on FrameworkTestAdapter (now removed). Follow-on session will rewrite
// the test to wire each target against its real sqlite adapter and
// re-add this runner.

#[test]
#[ignore]
fn errors_test_passes_under_crystal() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/errors_test.rb"),
        "errors",
    );
}

#[test]
#[ignore]
fn ac_base_test_passes_under_crystal() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/base_test.rb"),
        "ac_base",
    );
}

#[test]
#[ignore]
fn router_test_passes_under_crystal() {
    build_and_run(
        Path::new("runtime/ruby/test/action_dispatch/router_test.rb"),
        "router",
    );
}

#[test]
#[ignore]
fn view_helpers_test_passes_under_crystal() {
    build_and_run(
        Path::new("runtime/ruby/test/action_view/view_helpers_test.rb"),
        "view_helpers",
    );
}
