//! Swift toolchain integration test — emit-then-compile forcing function.
//!
//! Generates the emitted Swift/SPM project for real-blog into a scratch
//! directory and runs `swift build` against it — the Swift analog of
//! `kotlin_toolchain.rs`'s `gradle compileKotlin`: full parse + type-check
//! (+ link) of every emitted source against the real Hummingbird /
//! swift-nio / CSQLite dependency graph. Bare `swiftc` can't be used
//! because `Server.swift` imports Hummingbird and `Db.swift` imports
//! NIOPosix + the CSQLite systemLibrary, so the build has to go through
//! SPM (which resolves the declared dependencies).
//!
//! Requires a Swift toolchain (6+) on PATH and, on Linux, the
//! `libsqlite3-dev` package (the CSQLite systemLibrary's header + link
//! target — see docs/archive/swift-migration-plan.md decision 3). CI provides
//! both via `scripts/ci-swift-install` + apt; locally, the Xcode CLT
//! (macOS) or a swiftly/mise toolchain suffices.
//!
//! Marked `#[ignore]` so the default `cargo test` run stays fast. Run:
//!
//!     cargo test --test swift_toolchain -- --ignored --nocapture

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

use roundhouse::emit::swift;
use roundhouse::ingest::ingest_app;

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-swift-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    // Keep `.build/` across runs so SPM's resolved checkouts and compiled
    // dependency modules are reused locally (the emitted app sources are
    // tiny next to the Hummingbird/swift-nio tree); everything else is
    // regenerated from scratch.
    if out.exists() {
        for entry in std::fs::read_dir(out).expect("read scratch") {
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
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    // Emit from the post-analyze lowered IR, as `roundhouse --target swift` does
    roundhouse::session::analyze_and_lower(&mut app);
    let files = swift::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

/// `swift build` over the emitted project (debug configuration — the
/// type-check + link is the gate; release codegen adds minutes for no
/// extra verification).
fn run_swift_build(scratch: &Path) -> std::process::Output {
    let mut cmd = Command::new("swift");
    cmd.arg("build").current_dir(scratch);
    cmd.output().expect("run swift build")
}

fn assert_swift_builds(fixture: &str, scratch: &Path) {
    let output = run_swift_build(scratch);
    assert!(
        output.status.success(),
        "swift build failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_swift_builds() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    assert_swift_builds("real-blog", &scratch);
}

/// Execution leg: `swift test` over the emitted project runs the
/// TRANSPILED real-blog suite (article/comment model tests + controller
/// dispatch tests) under XCTest against an in-memory SQLite database —
/// compile-clean is necessary but not sufficient; this catches runtime
/// contract drift (fixture loading, Router.match dispatch, covariant
/// `class func` dispatch through inherited Base bodies).
///
/// Same prerequisites as the build leg PLUS XCTest, which Linux
/// toolchains bundle but the macOS Command Line Tools do NOT. On macOS,
/// point at a full Xcode per-invocation (no xcode-select needed):
///
///     DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
///         cargo test --test swift_toolchain -- --ignored --nocapture
#[test]
#[ignore]
fn real_blog_swift_tests_pass() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-test");
    generate_project(fixture, &scratch);

    let output = Command::new("swift")
        .arg("test")
        .current_dir(&scratch)
        .output()
        .expect("run swift test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "swift test failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        stdout,
        stderr,
    );

    // Defense against issue #4: `swift test` exits 0 when zero XCTest
    // methods are discovered. real-blog carries 21 tests across 4
    // suites; require a healthy floor so emit-routing can't silently
    // drop a whole test class.
    let executed = parse_executed_count(&stdout).or_else(|| parse_executed_count(&stderr));
    assert!(
        executed.map_or(false, |n| n >= 21),
        "expected >= 21 real-blog tests to run, got {executed:?}\n\
         stdout:\n{stdout}\nstderr:\n{stderr}",
    );
}

/// Extract N from the LAST XCTest "Executed N test(s)" summary line (the
/// all-tests rollup).
fn parse_executed_count(s: &str) -> Option<usize> {
    let idx = s.rfind("Executed ")?;
    let rest = &s[idx + "Executed ".len()..];
    let end = rest.find(' ')?;
    rest[..end].parse::<usize>().ok()
}

#[test]
#[ignore]
fn real_blog_i18n_locales_run() {
    // The answers `emit_and_run::i18n_locales` checks on CRuby, but for a String
    // locale named in an error: Swift has no Symbol apart from String, so `"de"` shows as `:de`.
    let (emitted, errors) = i18n_locales_app::probe_app()
        .edit("app/models/article.rb", i18n_locales_app::IN_LOCALE, "")
        .emit(roundhouse::project::BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("Tests/AppTests/I18nProbeTests.swift"), r#"import XCTest
@testable import App

final class I18nProbeTests: XCTestCase {
    func testLocalesAnswerAsRails() throws {
        XCTAssertEqual(try Article.lookups(), ["Hello", "Hi <x>", "1 item", "2 items", "Translation missing: en.probe.nope", "Cześć", "fallback 3", "en"])
        XCTAssertEqual(try Article.polish(), ["1 rzecz", "5 rzeczy", "English only", "Cześć", "Translation missing: pl.probe.nope", "Cześć", "Hi Ola", "pl"])
        XCTAssertEqual(I18n.locale(), "en")
        XCTAssertEqual([try Article.named("pl"), try Article.named("en"), try Article.named(nil)], ["Cześć", "Hello", "Hello"])
        XCTAssertEqual([try Article.probeKey("greet"), try Article.onlyKey("xx")], ["Hello", "Translation missing: en.probe.only_xx"])
        XCTAssertThrowsError(try Article.named("de")) { error in
            XCTAssertEqual((error as? InvalidLocale)?.message, ":de is not a valid locale")
        }
    }
}
"#).expect("write probe test");
    let output = Command::new("swift")
        .args(["test", "--filter", "I18nProbeTests"])
        .current_dir(&emitted)
        .output()
        .expect("run swift test");
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
    let (emitted, errors) = record_slice_app::probe_app().emit(roundhouse::project::BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("Tests/AppTests/RecordSliceProbeTests.swift"), r#"import XCTest
@testable import App

final class RecordSliceProbeTests: XCTestCase {
    func testSliceAnswersAsRails() throws {
        XCTAssertEqual(try Article.sliced(), ["2", "Sliced", "7", "1", "Other", "3", "Bound", "5", "4", "A comment body."])
    }
}
"#).expect("write probe test");
    let output = Command::new("swift")
        .args(["test", "--filter", "RecordSliceProbeTests"])
        .current_dir(&emitted)
        .output()
        .expect("run swift test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn real_blog_rescue_modifier_runs() {
    // A raise that throws is what `expr rescue fallback` catches; a nil answered without one is kept, as CRuby keeps it.
    let (emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.checked(s)\n    raise ArgumentError, \"empty\" if s.to_s.empty?\n    s.to_s\n  end\n\n  def self.safe(s)\n    checked(s) rescue \"none\"\n  end\n\n  def self.lookup(s)\n    raise ArgumentError, \"empty\" if s.to_s.empty?\n    s.to_s == \"gone\" ? nil : s.to_s\n  end\n\n  def self.safe_lookup(s)\n    lookup(s) rescue \"fallback\"\n  end\n\n  def self.strict(s)\n    checked(s) rescue raise(TypeError, \"bad\")\n  end\n",
        )
        .emit(roundhouse::project::BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("Tests/AppTests/RescueModifierTests.swift"), r#"import XCTest
@testable import App

final class RescueModifierTests: XCTestCase {
    func testTheFallbackAnswersARaise() throws {
        XCTAssertEqual(Article.safe("a") as? String, "a")
        XCTAssertEqual(Article.safe("") as? String, "none")
        XCTAssertNil(Article.safeLookup("gone"))
        XCTAssertEqual(Article.safeLookup("") as? String, "fallback")
        XCTAssertEqual(Article.safeLookup("a") as? String, "a")
        XCTAssertEqual(try Article.strict("a") as? String, "a")
        XCTAssertThrowsError(try Article.strict("")) { error in XCTAssertEqual((error as? TypeError)?.message, "bad") }
    }
}
"#).expect("write test");
    let output = Command::new("swift")
        .args(["test", "--filter", "RescueModifierTests"])
        .current_dir(&emitted)
        .output()
        .expect("run swift test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
