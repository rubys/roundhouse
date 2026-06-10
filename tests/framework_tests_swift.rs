//! Framework-test transpile gate (Swift target).
//!
//! Mirrors `framework_tests_kotlin.rs`: ingests one
//! `runtime/ruby/test/**/*_test.rb` file as a TestModule, drops it onto an
//! otherwise-empty App, runs `swift::emit`, and runs the emitted XCTest
//! class under `swift test`.
//!
//! What this catches that `swift_toolchain` (emit-then-compile of
//! real-blog) doesn't: transpile-fidelity gaps in the Ruby→Swift lowering
//! of the test file itself, plus Swift-runtime adapter-contract drift
//! surfaced by actually *running* assertions (compile-clean is necessary
//! but not sufficient). Sibling of the typescript / crystal / ruby /
//! kotlin gates.
//!
//! Requires a Swift toolchain (6+) and, on Linux, `libsqlite3-dev` — same
//! prerequisites as `swift_toolchain.rs`.
//!
//! Marked `#[ignore]` while gaps close — run explicitly:
//!
//!     cargo test --test framework_tests_swift -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::swift;
use roundhouse::ingest::ingest_test_file;
use roundhouse::App;

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
    base.join("roundhouse-framework-tests-swift").join(tag)
}

fn build_and_run(test_file: &Path, tag: &str) {
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

    let source = std::fs::read(test_file)
        .unwrap_or_else(|e| panic!("read {}: {e}", test_file.display()));
    let test_module = ingest_test_file(&source, &test_file.display().to_string())
        .expect("ingest framework test file")
        .expect("framework test file should contain a test class");

    let mut app = App::new();
    app.test_modules.push(test_module);
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
        "swift test failed for {} at {}:\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        scratch.display(),
        stdout,
        stderr,
    );

    assert_tests_ran(&stdout, &stderr, test_file);
}

/// Defense against issue #4: `swift test` exits 0 when zero XCTest
/// methods are discovered — if emit-routing dropped the test class, the
/// run would pass green having run nothing. Parse the XCTest summary
/// (`Executed N tests`) and require at least one.
fn assert_tests_ran(stdout: &str, stderr: &str, test_file: &Path) {
    let executed = parse_executed_count(stdout).or_else(|| parse_executed_count(stderr));
    assert!(
        executed.map_or(false, |n| n >= 1),
        "framework test for {} ran 0 tests — emit-routing likely dropped \
         the test class (see issue #4).\nstdout:\n{stdout}\nstderr:\n{stderr}",
        test_file.display(),
    );
}

/// Extract N from XCTest's "Executed N test(s)" summary line.
fn parse_executed_count(s: &str) -> Option<usize> {
    let idx = s.rfind("Executed ")?;
    let rest = &s[idx + "Executed ".len()..];
    let end = rest.find(' ')?;
    rest[..end].parse::<usize>().ok()
}

#[test]
#[ignore]
fn inflector_test_passes_under_swift() {
    build_and_run(Path::new("runtime/ruby/test/inflector_test.rb"), "inflector");
}

#[test]
#[ignore]
fn router_test_passes_under_swift() {
    build_and_run(
        Path::new("runtime/ruby/test/action_dispatch/router_test.rb"),
        "router",
    );
}

#[test]
#[ignore]
fn view_helpers_test_passes_under_swift() {
    build_and_run(
        Path::new("runtime/ruby/test/action_view/view_helpers_test.rb"),
        "view_helpers",
    );
}

#[test]
#[ignore]
fn errors_test_passes_under_swift() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/errors_test.rb"),
        "errors",
    );
}

#[test]
#[ignore]
fn ac_base_test_passes_under_swift() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/base_test.rb"),
        "ac_base",
    );
}
