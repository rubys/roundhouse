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

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
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
    Analyzer::new(&app).analyze(&mut app);
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
    let fixture = Path::new("fixtures/real-blog");
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
    let fixture = Path::new("fixtures/real-blog");
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
