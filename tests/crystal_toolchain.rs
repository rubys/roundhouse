//! Crystal toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted Crystal project for tiny-blog into a scratch
//! directory and runs `crystal build --no-codegen` against it. The
//! `--no-codegen` flag performs parsing + semantic analysis + type
//! checking without producing a binary, which is the Crystal
//! equivalent of `cargo check` — fast enough to be useful in a
//! test, and catches the same class of emit bugs.
//!
//! Marked `#[ignore]` so the default `cargo test` run stays fast.
//! Run explicitly:
//!
//!     cargo test --test crystal_toolchain -- --ignored --nocapture

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

use roundhouse::emit::crystal;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-crystal-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = crystal::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn shards_install(scratch: &Path) {
    // Per-test shards cache — the global ~/.cache/shards/ races on
    // git index.lock files when multiple installs run in parallel.
    let install = Command::new("shards")
        .arg("install")
        .current_dir(scratch)
        .env("SHARDS_CACHE_PATH", scratch.join(".shards-cache"))
        .output()
        .expect("run shards install");
    assert!(
        install.status.success(),
        "shards install failed at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );
}

fn run_crystal_build(scratch: &Path) -> std::process::Output {
    shards_install(scratch);
    Command::new("crystal")
        .arg("build")
        .arg("--no-codegen")
        .arg("src/app.cr")
        .current_dir(scratch)
        // Per-test build cache dir — Crystal's global ~/.cache/crystal
        // races when multiple parallel compiles touch overlapping
        // cache keys.
        .env("CRYSTAL_CACHE_DIR", scratch.join(".crystal-cache"))
        .output()
        .expect("run crystal build")
}

fn assert_crystal_passes(fixture: &str, scratch: &Path) {
    let output = run_crystal_build(scratch);
    assert!(
        output.status.success(),
        "crystal build --no-codegen failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn tiny_blog_crystal_build_passes() {
    let fixture = Path::new("fixtures/tiny-blog");
    let scratch = scratch_dir("tiny-blog");
    generate_project(fixture, &scratch);
    assert_crystal_passes("tiny-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_crystal_build_passes() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-build");
    generate_project(fixture, &scratch);
    assert_crystal_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_crystal_spec_passes() {
    // Phase 2 forcing function: emit real-blog, run `crystal spec`
    // against the generated project, assert zero failures. A subset
    // of tests are marked `pending` because they need persistence
    // runtime (Phase 3); the rest should pass.
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-spec");
    generate_project(fixture, &scratch);
    shards_install(&scratch);

    let output = Command::new("crystal")
        .arg("spec")
        .arg("--no-color")
        .current_dir(&scratch)
        .env("CRYSTAL_CACHE_DIR", scratch.join(".crystal-cache"))
        .output()
        .expect("run crystal spec");

    assert!(
        output.status.success(),
        "crystal spec failed on emitted real-blog at {}:\n\
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
    // The answers `emit_and_run::i18n_locales` checks on CRuby.
    let (emitted, errors) = i18n_locales_app::probe_app()
        .edit("app/models/article.rb", i18n_locales_app::IN_LOCALE, "")
        .emit(roundhouse::project::BuildTarget::Crystal);
    assert!(errors.is_empty(), "{errors:?}");
    shards_install(&emitted);
    std::fs::write(
        emitted.join("probe.cr"),
        r##"require "./src/app"
def expect(what, got, want)
  raise "#{what}: #{got.inspect}" unless got == want
end
expect("lookups", Article.lookups, ["Hello", "Hi <x>", "1 item", "2 items", "Translation missing: en.probe.nope", "Cześć", "fallback 3", "en"])
expect("polish", Article.polish, ["1 rzecz", "5 rzeczy", "English only", "Cześć", "Translation missing: pl.probe.nope", "Cześć", "Hi Ola", "pl"])
expect("restored", I18n.locale.to_s, "en")
expect("named", [Article.named("pl"), Article.named(:en), Article.named(nil)], ["Cześć", "Hello", "Hello"])
expect("dynamic", [Article.probe_key("greet"), Article.only_key("xx")], ["Hello", "Translation missing: en.probe.only_xx"])
begin
  Article.named("de")
  raise "no raise"
rescue e : I18n::InvalidLocale
  expect("named de", e.message, "\"de\" is not a valid locale")
end
puts "locales passed"
"##,
    )
    .unwrap();
    let run = Command::new("crystal")
        .args(["run", "probe.cr"])
        .current_dir(&emitted)
        .env("CRYSTAL_CACHE_DIR", emitted.join(".crystal-cache"))
        .output()
        .expect("run crystal");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success() && stdout.contains("locales passed"), "{stdout}\n{}", String::from_utf8_lossy(&run.stderr));
}

#[test]
#[ignore]
fn real_blog_record_slice_runs() {
    // The answers `emit_and_run::record_slice` checks on CRuby.
    let (emitted, errors) = record_slice_app::probe_app().emit(roundhouse::project::BuildTarget::Crystal);
    assert!(errors.is_empty(), "{errors:?}");
    shards_install(&emitted);
    std::fs::write(
        emitted.join("probe.cr"),
        r##"require "./src/app"
def expect(what, got, want)
  raise "#{what}: #{got.inspect}" unless got == want
end
expect("sliced", Article.sliced, ["2", "Sliced", "7", "1", "Other", "3", "Bound", "5", "4", "A comment body."])
puts "slice passed"
"##,
    )
    .unwrap();
    let run = Command::new("crystal")
        .args(["run", "probe.cr"])
        .current_dir(&emitted)
        .env("CRYSTAL_CACHE_DIR", emitted.join(".crystal-cache"))
        .output()
        .expect("run crystal");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success() && stdout.contains("slice passed"), "{stdout}\n{}", String::from_utf8_lossy(&run.stderr));
}
