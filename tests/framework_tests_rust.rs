//! Framework-test transpile gate (Rust target).
//!
//! Sibling of `framework_tests_crystal.rs` / `framework_tests_typescript.rs`
//! — ingests one `runtime/ruby/test/**/*_test.rb` file as a TestModule,
//! drops it onto an otherwise-empty App, runs `rust::emit` (the rust2
//! implementation), and invokes `cargo test` against the result.
//!
//! **Why this gate exists, concretely.** `JsonBuilder.encode_value`
//! dispatches `is_a?(TrueClass)` then `is_a?(FalseClass)`, and rust2
//! collapsed BOTH to `.is_boolean()` — every `false` serialized as
//! `true`. The typescript emitter had the identical bug, and it was
//! caught the day `json_builder_test` was wired to the typescript gate
//! (`db434d83`). rust had no framework gate, so nothing was ever going
//! to find it there; it was fixed only because the two emitters happened
//! to be wrong in the same way. That is the argument for this file:
//! every emitter renders the same framework Ruby differently, and
//! real-blog exercises only the slice of framework behavior a
//! scaffold blog reaches (it has no boolean column, so `compare rust`
//! stayed 7/7 throughout).
//!
//! What this catches that `framework_ruby_tests_pass` doesn't:
//! transpile-fidelity gaps in the Ruby→Rust lowering of the framework
//! runtime and of the test file itself, against real assertions rather
//! than against whatever real-blog happens to touch.
//!
//! Marked `#[ignore]` (cargo-in-cargo is slow) — run explicitly:
//!
//!     cargo test --test framework_tests_rust -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::App;
use roundhouse::analyze::Analyzer;
use roundhouse::emit::rust;
use roundhouse::ingest::ingest_test_file;

/// Walk `runtime/ruby/**/*.rbs` and merge each parsed signature into
/// `app.rbs_signatures`. Without this the test body-typer can't dispatch
/// precisely against framework methods, and the strict-typed Rust emit
/// falls through to the default `Ty::Untyped` collapse. Same helper as
/// the crystal/typescript gates (intentional duplication — each gate
/// stays self-contained).
fn load_framework_rbs(app: &mut App) {
    let runtime_ruby = Path::new("runtime/ruby");
    fn walk(dir: &Path, app: &mut App) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, app);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("rbs") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else { continue };
            let Ok(sigs) = roundhouse::rbs::parse_app_signatures(&source) else { continue };
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
    base.join("roundhouse-framework-tests-rust").join(tag)
}

fn build_and_run(test_file: &Path, tag: &str) {
    let scratch = scratch_dir(tag);
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(&scratch).expect("create scratch");

    let source =
        std::fs::read(test_file).unwrap_or_else(|e| panic!("read {}: {e}", test_file.display()));
    let test_module = ingest_test_file(&source, &test_file.display().to_string())
        .expect("ingest framework test file")
        .expect("framework test file should contain a test class");

    let mut app = App::new();
    app.test_modules.push(test_module);
    load_framework_rbs(&mut app);
    Analyzer::new(&app).analyze(&mut app);

    for file in rust::emit(&app) {
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    // Share the outer build's registry + a per-tag target dir: a cold
    // `cargo test` here would otherwise re-download and re-compile the
    // whole dependency graph for every suite.
    let output = Command::new("cargo")
        .arg("test")
        .arg("--quiet")
        .current_dir(&scratch)
        .env("CARGO_TARGET_DIR", scratch.join("target"))
        .output()
        .expect("run cargo test");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "cargo test failed for {} at {}:\n\
         === stdout ===\n{stdout}\n\
         === stderr ===\n{stderr}",
        test_file.display(),
        scratch.display(),
    );

    assert_tests_ran(&stdout, test_file, &scratch);
}

/// Defense against issue #4: `cargo test` exits 0 when the emitted crate
/// registers no `#[test]` fns — emit-routing that drops the `*Test`
/// class yields a crate that compiles and reports `0 passed`, green.
/// Sum every `test result: ok. N passed` line (one per test binary) and
/// require at least one.
fn assert_tests_ran(stdout: &str, test_file: &Path, scratch: &Path) {
    let count: usize = stdout.lines().filter_map(parse_passed).sum();
    assert!(
        count >= 1,
        "framework test for {} ran 0 tests — emit-routing likely dropped \
         the test class (see issue #4).\nscratch: {}\n=== stdout ===\n{}",
        test_file.display(),
        scratch.display(),
        stdout,
    );
    eprintln!("{}: {count} tests passed", test_file.display());
}

/// `test result: ok. 14 passed; 0 failed; …` → 14.
fn parse_passed(line: &str) -> Option<usize> {
    let rest = line.strip_prefix("test result: ok. ")?;
    let n = rest.split(' ').next()?;
    n.parse().ok()
}

#[test]
fn parses_the_cargo_summary() {
    assert_eq!(
        parse_passed("test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured"),
        Some(14)
    );
    assert_eq!(parse_passed("running 14 tests"), None);
}

#[test]
#[ignore]
fn inflector_test_passes_under_rust() {
    build_and_run(Path::new("runtime/ruby/test/inflector_test.rb"), "inflector");
}

// ── Deferred: wired, failing, and each on a NAMED rust2 gap ──────────
//
// These five run (`cargo test --test framework_tests_rust -- --ignored`)
// and fail to COMPILE the emitted crate — they are a worklist, not a
// mystery. Kept in the file so the intent is recorded and the next fix
// has something to run against; CI is scoped to the green subset, the
// same convention framework-tests-kotlin / -swift use. Drop names from
// the CI filter as these close. Tracked in #34 §1.
//
//   json_builder — the rust `JsonBuilder` surface is typed for APP call
//     sites, not the framework contract: `encode_value` takes
//     `serde_json::Value` so `encode_value(nil)` doesn't type, and
//     `encode_datetime` is generic over `EncodeDatetime`, which `&str`
//     doesn't implement and `None` can't infer. The Ruby contract
//     accepts a nil-or-String timestamp; the rust one accepts neither.
//
//   router — `raise` isn't in scope in a test module, and `Option<T>`
//     is dispatched against as if narrowed (`.is_null()`, `.action()`
//     on the `Option<MatchResult>` a `match` returns).
//
//   view_helpers — the hoisted inner `Article` emits, but without the
//     `id` accessor its own AR-shaped body implies, and String/Option
//     mismatches remain (`unwrap_or_default` on a `String`).
//
//   errors — `RecordNotFound` / `StandardError` as class-reflection
//     values. THE SAME FAMILY kotlin and swift defer; whoever closes it
//     for one should check all three in the same pass.
//
//   ac_base — the inline `TestController` body, dominated by 54
//     "expected value, found module `self`". Also a kotlin/swift
//     sibling.

#[test]
#[ignore]
fn json_builder_test_passes_under_rust() {
    build_and_run(Path::new("runtime/ruby/test/json_builder_test.rb"), "json_builder");
}

#[test]
#[ignore]
fn router_test_passes_under_rust() {
    build_and_run(Path::new("runtime/ruby/test/action_dispatch/router_test.rb"), "router");
}

#[test]
#[ignore]
fn view_helpers_test_passes_under_rust() {
    build_and_run(Path::new("runtime/ruby/test/action_view/view_helpers_test.rb"), "view_helpers");
}

#[test]
#[ignore]
fn errors_test_passes_under_rust() {
    build_and_run(Path::new("runtime/ruby/test/active_record/errors_test.rb"), "errors");
}

#[test]
#[ignore]
fn ac_base_test_passes_under_rust() {
    build_and_run(Path::new("runtime/ruby/test/action_controller/base_test.rb"), "ac_base");
}

// The five suites testing framework files no target's runtime
// transpiles (slots, registry, cookies, action_text, date_helper) are
// deliberately absent — see the note in `framework_tests_crystal.rs`
// and #34 §1. `active_record/base_test` is the seventh reachable file,
// behind the per-target SqliteAdapter work.
