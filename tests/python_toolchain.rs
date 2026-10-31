//! Python toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted Python project and runs `python -m compileall`
//! over the models package to verify syntax. Python's dynamic
//! late-binding means undefined names don't fail compile (they fail at
//! run time), so compileall is the honest pre-test check available
//! without third-party tools.
//!
//! Marked `#[ignore]`. Explicit:
//!
//!     cargo test --test python_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/i18n_locales_app.rs"]
#[allow(dead_code)]
mod i18n_locales_app;
#[path = "support/record_slice_app.rs"]
#[allow(dead_code)]
mod record_slice_app;

use roundhouse::emit::python;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-python-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    // Emit from the post-analyze lowered IR, as `roundhouse --target python` does
    roundhouse::session::analyze_and_lower(&mut app);
    let files = python::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn assert_python_compiles(fixture: &str, scratch: &Path) {
    // Compile-check the whole `app/` package. Phase 4c brought
    // controllers + the http stub under compileall; earlier phases
    // scoped this to `app/models.py`.
    let output = Command::new("python3")
        .arg("-m")
        .arg("compileall")
        .arg("-q")
        .arg("app/")
        .current_dir(scratch)
        .output()
        .expect("run python compileall");

    assert!(
        output.status.success(),
        "python compileall failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn tiny_blog_python_compile_passes() {
    let fixture = Path::new("fixtures/tiny-blog");
    let scratch = scratch_dir("tiny-blog");
    generate_project(fixture, &scratch);
    assert_python_compiles("tiny-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_python_compile_passes() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    assert_python_compiles("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_python_unittest_passes() {
    // Phase 2 forcing function: emit real-blog, run `python -m
    // unittest discover tests`, assert zero failures. Phase-3 tests
    // are @unittest.skip'd.
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("python3")
        .arg("-m")
        .arg("unittest")
        .arg("discover")
        .arg("-s")
        .arg("tests")
        .arg("-t")
        .arg(".")
        .current_dir(&scratch)
        .output()
        .expect("run python unittest");

    assert!(
        output.status.success(),
        "python unittest failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_i18n_locales_run() {
    // The answers `emit_and_run::i18n_locales` checks on CRuby, but for a String
    // locale named in an error: Python has no Symbol apart from str, so `"de"` shows as `:de`.
    let (emitted, errors) = i18n_locales_app::probe_app()
        .edit("app/models/article.rb", i18n_locales_app::IN_LOCALE, "")
        .emit(roundhouse::project::BuildTarget::Python);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("probe.py"), r#"from app.v2.models import Article
from app.i18n_locale import I18n, InvalidLocale

def expect(what, got, want):
    if got != want:
        raise AssertionError(f"{what}: {got!r}")

expect("lookups", Article.lookups(), ["Hello", "Hi <x>", "1 item", "2 items", "Translation missing: en.probe.nope", "Cześć", "fallback 3", "en"])
expect("polish", Article.polish(), ["1 rzecz", "5 rzeczy", "English only", "Cześć", "Translation missing: pl.probe.nope", "Cześć", "Hi Ola", "pl"])
expect("restored", I18n.locale(), "en")
expect("named", [Article.named("pl"), Article.named("en"), Article.named(None)], ["Cześć", "Hello", "Hello"])
expect("dynamic", [Article.probe_key("greet"), Article.only_key("xx")], ["Hello", "Translation missing: en.probe.only_xx"])
try:
    Article.named("de")
    raise AssertionError("no raise")
except InvalidLocale as e:
    expect("named de", str(e), ":de is not a valid locale")
print("locales passed")
"#).expect("write probe");
    let output = Command::new("python3").arg("probe.py").current_dir(&emitted).output().expect("run python3");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("locales passed"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore]
fn real_blog_record_slice_runs() {
    // The answers `emit_and_run::record_slice` checks on CRuby.
    let (emitted, errors) = record_slice_app::probe_app().emit(roundhouse::project::BuildTarget::Python);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("probe.py"), r#"from app.v2.models import Article

got = Article.sliced()
if got != ["2", "Sliced", "7", "1", "Other", "3", "Bound", "5", "4", "A comment body."]:
    raise AssertionError(f"sliced: {got!r}")
print("slice passed")
"#).expect("write probe");
    let output = Command::new("python3").arg("probe.py").current_dir(&emitted).output().expect("run python3");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("slice passed"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}
