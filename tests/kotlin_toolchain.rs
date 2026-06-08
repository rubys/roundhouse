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
