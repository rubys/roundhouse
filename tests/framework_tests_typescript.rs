//! Framework-test transpile gate (TypeScript target).
//!
//! Transpiles `runtime/ruby/test/**/*_test.rb` to TypeScript and runs
//! the result under `tsx`. Catches a class of bugs the source-side
//! framework_ruby_tests_pass gate can't see: per-target adapter-
//! contract drift, transpile-fidelity gaps, target-runtime semantic
//! divergence.
//!
//! Programmatic test bed (no fixture directory): we pick a single
//! framework test file, ingest it as a `TestModule`, drop it onto an
//! otherwise-empty `App`, run the standard emit pipeline, then
//! invoke `tsx --test test/*.test.ts`. The emitted project picks up
//! the same framework-runtime files (`runtime/typescript/*.ts` +
//! transpiled `runtime/ruby/*.rb`) the real-blog gate uses.
//!
//! Marked `#[ignore]` while gaps close — run explicitly:
//!
//!     cargo test --test framework_tests_typescript -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::typescript;
use roundhouse::ingest::ingest_test_file;
use roundhouse::App;

/// Walk `runtime/ruby/**/*.rbs` and merge each parsed signature into
/// `app.rbs_signatures`. The framework-runtime RBS sidecars carry
/// per-method types the body-typer needs to dispatch precisely
/// across the test body — without this, calls like
/// `ActionDispatch::Router.match(...)` resolve to `Untyped` and
/// downstream `m[:path_params].length` can't pick the Hash dispatch.
/// `ingest_app` already does this for full Rails-shaped fixtures;
/// the framework_tests_* gates start from an empty App and have to
/// load the framework RBS explicitly.
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
    base.join("roundhouse-framework-tests").join(tag)
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

    for file in typescript::emit(&app) {
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    let install = Command::new("npm")
        .arg("install")
        .arg("--silent")
        .arg("--no-audit")
        .arg("--no-fund")
        .current_dir(&scratch)
        .output()
        .expect("run npm install");
    assert!(
        install.status.success(),
        "npm install failed at {}:\n=== stdout ===\n{}\n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    let output = Command::new("sh")
        .arg("-c")
        .arg("./node_modules/.bin/tsx --test test/*.test.ts")
        .current_dir(&scratch)
        .output()
        .expect("run tsx --test");

    assert!(
        output.status.success(),
        "tsx --test failed for {} at {}:\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn ar_base_test_passes_under_tsx() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/base_test.rb"),
        "ar_base",
    );
}

#[test]
#[ignore]
fn errors_test_passes_under_tsx() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/errors_test.rb"),
        "errors",
    );
}

#[test]
#[ignore]
fn parameters_test_passes_under_tsx() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/parameters_test.rb"),
        "parameters",
    );
}

#[test]
#[ignore]
fn ac_base_test_passes_under_tsx() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/base_test.rb"),
        "ac_base",
    );
}

#[test]
#[ignore]
fn router_test_passes_under_tsx() {
    build_and_run(
        Path::new("runtime/ruby/test/action_dispatch/router_test.rb"),
        "router",
    );
}

#[test]
#[ignore]
fn view_helpers_test_passes_under_tsx() {
    build_and_run(
        Path::new("runtime/ruby/test/action_view/view_helpers_test.rb"),
        "view_helpers",
    );
}

#[test]
#[ignore]
fn inflector_test_passes_under_tsx() {
    build_and_run(
        Path::new("runtime/ruby/test/inflector_test.rb"),
        "inflector",
    );
}
