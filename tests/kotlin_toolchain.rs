//! Kotlin toolchain integration test — emit-then-compile forcing function.
//!
//! Generates the emitted Kotlin/Gradle project for real-blog into a scratch
//! directory and runs `gradle compileKotlin` against it — the Kotlin analog
//! of `crystal build --no-codegen` (`crystal_toolchain.rs`): full parse +
//! type-check of every emitted source against the real Javalin / sqlite-jdbc
//! classpath, without producing the final distribution. Bare `kotlinc` can't
//! be used because `Server.kt` imports `io.javalin.*`, so the build has to go
//! through Gradle (which resolves the declared dependencies).
//!
//! Requires a JDK (17+) and a `gradle` on PATH. CI provides both via
//! `actions/setup-java` + `gradle/actions/setup-gradle`; locally, set
//! `JAVA_HOME` (or `KOTLIN_JAVA_HOME`) and have `gradle` installed.
//!
//! Marked `#[ignore]` so the default `cargo test` run stays fast. Run:
//!
//!     cargo test --test kotlin_toolchain -- --ignored --nocapture

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/i18n_locales_app.rs"]
#[allow(dead_code)]
mod i18n_locales_app;
#[path = "support/record_slice_app.rs"]
#[allow(dead_code)]
mod record_slice_app;

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::kotlin;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-kotlin-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = kotlin::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

/// `gradle compileKotlin` over the emitted project. Honors `KOTLIN_JAVA_HOME`
/// then `JAVA_HOME` for the JDK. Uses the default `GRADLE_USER_HOME` so CI's
/// `setup-gradle` dependency cache applies (this test is the only Gradle
/// consumer, so there's no parallel cache race to isolate).
fn run_gradle_compile(scratch: &Path) -> std::process::Output {
    let java_home = std::env::var("KOTLIN_JAVA_HOME")
        .or_else(|_| std::env::var("JAVA_HOME"))
        .ok();

    let mut cmd = Command::new("gradle");
    cmd.arg("compileKotlin")
        .arg("--console=plain")
        .arg("--no-daemon")
        .arg("-q")
        .current_dir(scratch);
    if let Some(jh) = java_home {
        cmd.env("JAVA_HOME", jh);
    }
    cmd.output().expect("run gradle compileKotlin")
}

fn assert_kotlin_compiles(fixture: &str, scratch: &Path) {
    let output = run_gradle_compile(scratch);
    assert!(
        output.status.success(),
        "gradle compileKotlin failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_kotlin_compiles() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    assert_kotlin_compiles("real-blog", &scratch);
}

/// Execution leg: `gradle test` over the emitted project runs the
/// TRANSPILED real-blog suite (article/comment model tests + controller
/// dispatch tests) under JUnit 5 against an in-memory SQLite database —
/// compile-clean is necessary but not sufficient; this catches runtime
/// contract drift (fixture loading, Router.match dispatch, the
/// RoundhouseTestCase harness). Sibling of swift's
/// `real_blog_swift_tests_pass` and the crystal/rust/go/ruby/python/
/// elixir/typescript execution legs.
#[test]
#[ignore]
fn real_blog_kotlin_tests_pass() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-test");
    generate_project(fixture, &scratch);

    let java_home = std::env::var("KOTLIN_JAVA_HOME")
        .or_else(|_| std::env::var("JAVA_HOME"))
        .ok();
    let mut cmd = Command::new("gradle");
    cmd.arg("test").arg("--console=plain").arg("--no-daemon").current_dir(&scratch);
    if let Some(jh) = java_home {
        cmd.env("JAVA_HOME", jh);
    }
    let output = cmd.output().expect("run gradle test");
    assert!(
        output.status.success(),
        "gradle test failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Defense against issue #4: `gradle test` exits 0 when the test
    // source set discovers no JUnit classes. real-blog carries 21 tests
    // across 4 suites; require the full floor so emit-routing can't
    // silently drop a test class.
    let results_dir = scratch.join("build/test-results/test");
    let mut total = 0usize;
    if let Ok(entries) = std::fs::read_dir(&results_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("xml") {
                continue;
            }
            if let Ok(xml) = std::fs::read_to_string(&path) {
                total += parse_testsuite_count(&xml);
            }
        }
    }
    assert!(
        total >= 21,
        "expected >= 21 real-blog tests to run, got {total}\nresults dir: {}",
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
fn real_blog_i18n_locales_run() {
    // The answers `emit_and_run::i18n_locales` checks on CRuby, but for a String
    // locale named in an error: Kotlin has no Symbol apart from String, so `"de"` shows as `:de`.
    let (emitted, errors) = i18n_locales_app::probe_app()
        .edit("app/models/article.rb", i18n_locales_app::IN_LOCALE, "")
        .emit(roundhouse::project::BuildTarget::Kotlin);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("src/test/kotlin/I18nProbeTest.kt"), r#"package roundhouse

class I18nProbeTest {
    private fun expect(what: String, got: Any?, want: Any?) {
        if (got != want) throw AssertionError("$what: $got")
    }

    @org.junit.jupiter.api.Test
    fun localesAnswerAsRails() {
        expect("lookups", Article.lookups(), listOf("Hello", "Hi <x>", "1 item", "2 items", "Translation missing: en.probe.nope", "Cześć", "fallback 3", "en"))
        expect("polish", Article.polish(), listOf("1 rzecz", "5 rzeczy", "English only", "Cześć", "Translation missing: pl.probe.nope", "Cześć", "Hi Ola", "pl"))
        expect("restored", I18n.locale(), "en")
        expect("named", listOf(Article.named("pl"), Article.named("en"), Article.named(null)), listOf("Cześć", "Hello", "Hello"))
        expect("dynamic", listOf(Article.probeKey("greet"), Article.onlyKey("xx")), listOf("Hello", "Translation missing: en.probe.only_xx"))
        val e = org.junit.jupiter.api.assertThrows<InvalidLocale> { Article.named("de") }
        expect("named de", e.message, ":de is not a valid locale")
    }
}
"#).expect("write probe test");
    let mut cmd = Command::new("gradle");
    cmd.args(["test", "--tests", "roundhouse.I18nProbeTest", "--console=plain", "--no-daemon", "-q"]).current_dir(&emitted);
    if let Ok(jh) = std::env::var("KOTLIN_JAVA_HOME").or_else(|_| std::env::var("JAVA_HOME")) {
        cmd.env("JAVA_HOME", jh);
    }
    let output = cmd.output().expect("run gradle test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn real_blog_record_slice_runs() {
    // The answers `emit_and_run::record_slice` checks on CRuby.
    let (emitted, errors) = record_slice_app::probe_app().emit(roundhouse::project::BuildTarget::Kotlin);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("src/test/kotlin/RecordSliceProbeTest.kt"), r#"package roundhouse

class RecordSliceProbeTest {
    @org.junit.jupiter.api.Test
    fun sliceAnswersAsRails() {
        val got = Article.sliced()
        if (got != listOf("2", "Sliced", "7", "1", "Other", "3", "Bound", "5", "4", "A comment body.")) throw AssertionError("sliced: $got")
    }
}
"#).expect("write probe test");
    let mut cmd = Command::new("gradle");
    cmd.args(["test", "--tests", "roundhouse.RecordSliceProbeTest", "--console=plain", "--no-daemon", "-q"]).current_dir(&emitted);
    if let Ok(jh) = std::env::var("KOTLIN_JAVA_HOME").or_else(|_| std::env::var("JAVA_HOME")) {
        cmd.env("JAVA_HOME", jh);
    }
    let output = cmd.output().expect("run gradle test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
