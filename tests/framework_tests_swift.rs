//! Framework-test transpile gate (Swift target).
//!
//! Ingests the five wired `runtime/ruby/test/**/*_test.rb` files as
//! TestModules in an otherwise-empty App, runs `swift::emit`, and runs
//! all emitted XCTest classes under one `swift test`. This compiles the
//! shared runtime and SPM dependencies once rather than five times.
//!
//! What this catches that `swift_toolchain` (emit-then-compile of
//! real-blog) doesn't: transpile-fidelity gaps in the Ruby→Swift lowering
//! of the test file itself, plus Swift-runtime adapter-contract drift
//! surfaced by actually *running* assertions (compile-clean is necessary
//! but not sufficient). Sibling of the typescript / crystal / ruby /
//! kotlin gates.
//!
//! Requires a Swift toolchain (6+) and, on Linux, `libsqlite3-dev` — same
//! prerequisites as `swift_toolchain.rs` — PLUS XCTest, which Linux
//! toolchains bundle but the macOS Command Line Tools do NOT. On macOS,
//! point at a full Xcode per-invocation (no xcode-select needed):
//!
//!     DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
//!         cargo test --test framework_tests_swift -- --ignored --nocapture
//!
//! Marked `#[ignore]` while gaps close — run explicitly as above.

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::App;
use roundhouse::analyze::Analyzer;
use roundhouse::emit::swift;
use roundhouse::ingest::ingest_test_file;

/// Walk `runtime/ruby/**/*.rbs` and merge each parsed signature into
/// `app.rbs_signatures`. Without this the test body-typer can't dispatch
/// precisely against framework methods (`Inflector.pluralize`, …). Same
/// helper as the typescript/crystal/kotlin gates (intentional duplication —
/// keeping each gate self-contained).
fn load_framework_rbs(app: &mut App) {
    let runtime_ruby = Path::new("runtime/ruby");
    fn walk(dir: &Path, app: &mut App) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, app);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("rbs") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(sigs) = roundhouse::rbs::parse_app_signatures(&source) else {
                continue;
            };
            for (class_id, methods) in sigs {
                app.rbs_signatures
                    .entry(class_id)
                    .or_default()
                    .extend(methods);
            }
        }
    }
    walk(runtime_ruby, app);
}

fn scratch_dir(tag: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse-framework-tests-swift").join(tag)
}

fn build_and_run(test_files: &[&str], tag: &str) {
    let scratch = scratch_dir(tag);
    // Keep `.build/` (the SPM dependency tree) across runs; regenerate
    // everything else — same policy as swift_toolchain.rs.
    if scratch.exists() {
        for entry in std::fs::read_dir(&scratch).expect("read scratch") {
            let entry = entry.expect("scratch entry");
            if entry.file_name() == ".build" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path).expect("clean scratch entry");
            } else {
                std::fs::remove_file(&path).expect("clean scratch file");
            }
        }
    }
    std::fs::create_dir_all(&scratch).expect("create scratch");

    let mut app = App::new();
    for test_file in test_files {
        let source = std::fs::read(test_file).unwrap_or_else(|e| panic!("read {test_file}: {e}"));
        let test_module = ingest_test_file(&source, test_file)
            .expect("ingest framework test file")
            .expect("framework test file should contain a test class");
        app.test_modules.push(test_module);
    }
    load_framework_rbs(&mut app);
    Analyzer::new(&app).analyze(&mut app);

    for file in swift::emit(&app) {
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    // `swift test` builds main + tests and runs XCTest.
    let output = Command::new("swift")
        .arg("test")
        .current_dir(&scratch)
        .output()
        .expect("run swift test");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "swift framework tests failed at {}:\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        scratch.display(),
        stdout,
        stderr,
    );

    for test_module in &app.test_modules {
        assert_tests_ran(&stdout, &stderr, test_module.name.0.as_str());
    }
}

/// Defense against issue #4: `swift test` exits 0 when zero XCTest
/// methods are discovered — if emit-routing dropped the test class, the
/// run would pass green. Require at least one test in EACH suite, not
/// just a nonzero total that could hide a missing class.
fn assert_tests_ran(stdout: &str, stderr: &str, suite: &str) {
    let executed =
        parse_executed_count(stdout, suite).or_else(|| parse_executed_count(stderr, suite));
    assert!(
        executed.map_or(false, |n| n >= 1),
        "framework suite {suite} ran 0 tests — emit-routing likely dropped \
         the test class (see issue #4).\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
}

/// Extract N from the summary immediately following this suite's result.
fn parse_executed_count(s: &str, suite: &str) -> Option<usize> {
    let marker = format!("Test Suite '{suite}' passed");
    let summary = s
        .lines()
        .skip_while(|line| !line.starts_with(&marker))
        .nth(1)?;
    let rest = summary.trim().strip_prefix("Executed ")?;
    let end = rest.find(' ')?;
    rest[..end].parse::<usize>().ok()
}

#[test]
fn xctest_counts_are_per_suite_not_the_total() {
    let output = "Test Suite 'InflectorTest' passed at 2026-09-30\n\
        \t Executed 4 tests, with 0 failures\n\
        Test Suite 'RouterTest' passed at 2026-09-30\n\
        \t Executed 0 tests, with 0 failures\n\
        Test Suite 'All tests' passed at 2026-09-30\n\
        \t Executed 4 tests, with 0 failures\n";
    assert_eq!(parse_executed_count(output, "InflectorTest"), Some(4));
    assert_eq!(parse_executed_count(output, "RouterTest"), Some(0));
    assert_eq!(parse_executed_count(output, "ViewHelpersTest"), None);
    assert_tests_ran(output, "", "InflectorTest");
    assert_tests_ran("", output, "InflectorTest");
    for suite in ["RouterTest", "ViewHelpersTest"] {
        assert!(std::panic::catch_unwind(|| assert_tests_ran(output, "", suite)).is_err());
    }
}

// errors + ac_base were the last deferred pair; both are green now and CI
// runs this file unfiltered. What it took, recorded because kotlin needed
// the same four fixes and rust still does:
//   - errors:  `RecordNotFound < StandardError` is Ruby class-reflection,
//     not value comparison. Swift's metatype `is` covers both halves of
//     what Ruby means here — `Child.self is Parent.Type` for inheritance,
//     `RecordNotFound.self is Error.Type` for the protocol conformance a
//     `< StandardError` transpile actually becomes.
//   - ac_base: the inline `TestController < ActionController::Base` is
//     ingested as a plain LibraryClass, so none of the controller
//     lowering runs. Fixed at the LOWERING (shared by every target): an
//     inner class now inherits its parent's instance surface and takes
//     the parent's signature for a same-arity override.
//
// STILL VACUOUS, and not a swift gap: `assert_raises(NotImplementedError)
// { … }` emits as `/* TODO BeginRescue */`, so
// `testBaseProcessActionRaisesWhenNotOverridden` asserts nothing. Same on
// kotlin. Tracked in roundhouse#34.
//
// The errors file's SECOND top-level test class (`RecordInvalidTest`) is
// still dropped by ingest's single-`*Test`-class pick — see the note in
// `src/ingest/test.rs`. That drop is cross-target (crystal and typescript
// run the same 3 of 9 tests), and reaching it needs `Struct.new(:errors)`
// support, not wiring.
#[test]
#[ignore]
fn framework_tests_pass_under_swift() {
    build_and_run(
        &[
            "runtime/ruby/test/inflector_test.rb",
            "runtime/ruby/test/action_dispatch/router_test.rb",
            "runtime/ruby/test/action_view/view_helpers_test.rb",
            "runtime/ruby/test/active_record/errors_test.rb",
            "runtime/ruby/test/action_controller/base_test.rb",
        ],
        "all",
    );
}
