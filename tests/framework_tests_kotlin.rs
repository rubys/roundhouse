//! Framework-test transpile gate (Kotlin target).
//!
//! Mirrors `framework_tests_typescript.rs` / `framework_tests_crystal.rs`:
//! ingests one `runtime/ruby/test/**/*_test.rb` file as a TestModule, drops
//! it onto an otherwise-empty App, runs `kotlin::emit`, and runs the emitted
//! JUnit-5 spec under `gradle test`.
//!
//! What this catches that `kotlin_toolchain` (emit-then-compile of real-blog)
//! doesn't: transpile-fidelity gaps in the Ruby→Kotlin lowering of the test
//! file itself, plus Kotlin-runtime adapter-contract drift surfaced by
//! actually *running* assertions (compile-clean is necessary but not
//! sufficient). Sibling of the typescript / crystal / ruby / spinel gates.
//!
//! Requires a JDK (17+) and `gradle` on PATH — same prerequisites as
//! `kotlin_toolchain.rs`. CI provides both via `actions/setup-java` +
//! `gradle/actions/setup-gradle`.
//!
//! Marked `#[ignore]` while gaps close — run explicitly:
//!
//!     cargo test --test framework_tests_kotlin -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use roundhouse::analyze::Analyzer;
use roundhouse::emit::kotlin;
use roundhouse::ingest::ingest_test_file;
use roundhouse::App;

/// Walk `runtime/ruby/**/*.rbs` and merge each parsed signature into
/// `app.rbs_signatures`. Without this the test body-typer can't dispatch
/// precisely against framework methods (`Inflector.pluralize`, …) and the
/// strict-typed Kotlin emit falls through to the `Ty::Untyped → Any?`
/// collapse. Same helper as the typescript/crystal gates (intentional
/// duplication — keeping each gate self-contained).
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
    base.join("roundhouse-framework-tests-kotlin").join(tag)
}

fn gradle_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn build_and_run(test_file: &Path, tag: &str) {
    let _gradle_lock = gradle_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    for file in kotlin::emit(&app) {
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    // `gradle test` compiles main + test and runs the JUnit platform.
    // Honors `KOTLIN_JAVA_HOME` then `JAVA_HOME` for the JDK (matches
    // `kotlin_toolchain.rs`). Default `GRADLE_USER_HOME` so CI's
    // `setup-gradle` dependency cache applies.
    let java_home = std::env::var("KOTLIN_JAVA_HOME")
        .or_else(|_| std::env::var("JAVA_HOME"))
        .ok();
    let mut cmd = Command::new("gradle");
    cmd.arg("test")
        .arg("--console=plain")
        .arg("--no-daemon")
        .current_dir(&scratch);
    if let Some(jh) = java_home {
        cmd.env("JAVA_HOME", jh);
    }
    let output = cmd.output().expect("run gradle test");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "gradle test failed for {} at {}:\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        scratch.display(),
        stdout,
        stderr,
    );

    assert_tests_ran(&scratch, test_file);
}

/// Defense against issue #4: `gradle test` exits 0 when the test source set
/// is empty (no JUnit classes discovered) — if emit-routing dropped the test
/// class, the build would pass green having run nothing. Parse the JUnit XML
/// result files (`build/test-results/test/TEST-*.xml`) and require at least
/// one test was executed across them.
fn assert_tests_ran(scratch: &Path, test_file: &Path) {
    let results_dir = scratch.join("build/test-results/test");
    let mut total = 0usize;
    if let Ok(entries) = std::fs::read_dir(&results_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("xml") {
                continue;
            }
            let Ok(xml) = std::fs::read_to_string(&path) else {
                continue;
            };
            total += parse_testsuite_count(&xml);
        }
    }
    assert!(
        total >= 1,
        "framework test for {} ran 0 tests — emit-routing likely dropped \
         the test class (see issue #4).\nresults dir: {}",
        test_file.display(),
        results_dir.display(),
    );
}

/// Extract the `tests="N"` attribute from a JUnit `<testsuite …>` element.
fn parse_testsuite_count(xml: &str) -> usize {
    let Some(idx) = xml.find("<testsuite ") else {
        return 0;
    };
    let tail = &xml[idx..];
    let Some(attr_idx) = tail.find("tests=\"") else {
        return 0;
    };
    let rest = &tail[attr_idx + "tests=\"".len()..];
    let end = rest.find('"').unwrap_or(0);
    rest[..end].parse::<usize>().unwrap_or(0)
}

#[test]
#[ignore]
fn inflector_test_passes_under_kotlin() {
    build_and_run(Path::new("runtime/ruby/test/inflector_test.rb"), "inflector");
}

#[test]
#[ignore]
fn router_test_passes_under_kotlin() {
    build_and_run(
        Path::new("runtime/ruby/test/action_dispatch/router_test.rb"),
        "router",
    );
}

// view_helpers exercises an inline `Article < ActiveRecord::Base` helper
// declared in test scope. Greened by emitting the test module's
// `inner_classes` above the test body (the companion-hoist the
// typescript/crystal gates already do) plus pinning the `[]`/`[]=`
// override key param to String so it resolves against the AR base indexer.
#[test]
#[ignore]
fn view_helpers_test_passes_under_kotlin() {
    build_and_run(
        Path::new("runtime/ruby/test/action_view/view_helpers_test.rb"),
        "view_helpers",
    );
}

// The two gates below still FAIL on deeper kotlin emit gaps (NOT wiring —
// the inner helper classes now emit, but their bodies hit target-specific
// gaps the inflector/router/view_helpers slices don't):
//   - errors:  `RecordNotFound < StandardError` is Ruby class-reflection
//     (emitted as a `<` / `compareTo` over unresolved `StandardError`), and
//     the file's second top-level `*Test` class is dropped by ingest's
//     single-test-class pick.
//   - ac_base: the inline `TestController < ActionController::Base` body
//     surfaces several gaps at once — `processAction` override signature,
//     `toSym`, params `Map`-vs-`String` typing, String method mapping.
// Both pass under the typescript/crystal gates, so there's a reference
// shape to port. Until then CI runs the green subset (inflector + router +
// view_helpers) — same convention as toolchain-typescript's tiny_blog
// filter. Tracked in roundhouse#34. Run all five locally with:
//   cargo test --test framework_tests_kotlin -- --ignored
#[test]
#[ignore]
fn errors_test_passes_under_kotlin() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/errors_test.rb"),
        "errors",
    );
}

#[test]
#[ignore]
fn ac_base_test_passes_under_kotlin() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/base_test.rb"),
        "ac_base",
    );
}
