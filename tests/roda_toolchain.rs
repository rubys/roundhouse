//! Toolchain gate for the Roda + Sequel fixture (`fixtures/roda-blog`),
//! the step-1 exemplar of issue #67.
//!
//! The fixture is vendored from <https://github.com/rubys/roda-sequel-blog>
//! (`git archive` of the tracked tree — re-vendor after upstream changes).
//! Its `test/blog_test.rb` is the behavioral oracle: an in-process
//! minitest + rack-test spec of the full route surface.
//!
//! Two gates, in the order they come online:
//!
//! 1. `roda_blog_oracle_passes` (below) — the oracle suite is green
//!    against the fixture as-is under MRI. This pins the spec the
//!    transpiler is built against; if the fixture and suite ever drift,
//!    this fails before any roundhouse work is misattributed.
//!
//! 2. `roda_blog_transpiled_oracle_passes` — ingest `fixtures/roda-blog`
//!    through the Roda/Sequel recognizers, emit the CRuby target, and
//!    run the PORTED oracle (`tests/roda_oracle/blog_oracle_test.rb` —
//!    same 19 checks, driving the emitted Rack app through Rack::Test
//!    and the emitted AR-shaped model API) against the emitted app.
//!    The analog of `real_blog_spinel_tests_pass` in
//!    `ruby_toolchain.rs`. A check failing here while passing in gate 1
//!    is a transpiler defect by definition.
//!
//! Like the other toolchain tests this is `#[ignore]`d: it shells out to
//! bundler/MRI, which is a CI (or explicit `--ignored`) concern.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Ruby-target transpile of the fixture into a scratch tree —
/// mirrors the CLI's `--target ruby` flow (ingest → analyze+lower →
/// `project::target_files`).
fn generate_ruby_project(fixture: &Path, scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(scratch).expect("create scratch");
    let mut app = roundhouse::ingest::ingest_app(fixture).expect("ingest roda-blog");
    roundhouse::session::analyze_and_lower(&mut app);
    let files =
        roundhouse::project::target_files(&app, fixture, roundhouse::project::BuildTarget::Ruby)
            .expect("ruby target_files");
    for (path, content) in files {
        let out = scratch.join(&path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).expect("mkdir emit parent");
        }
        std::fs::write(&out, content).expect("write emitted file");
    }
}

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-ruby-{tag}"))
}

#[test]
#[ignore]
fn roda_blog_oracle_passes() {
    let fixture = std::fs::canonicalize(Path::new("fixtures/roda-blog")).expect("fixture dir");
    let gemfile = fixture.join("Gemfile");

    let install = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .arg("install")
        .arg("--quiet")
        .current_dir(&fixture)
        .output()
        .expect("spawn bundle install");
    assert!(
        install.status.success(),
        "bundle install failed\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .arg("exec")
        .arg("ruby")
        .arg("test/blog_test.rb")
        .current_dir(&fixture)
        .output()
        .expect("spawn ruby");
    assert!(
        output.status.success(),
        "roda-blog oracle failed\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn roda_blog_transpiled_oracle_passes() {
    let fixture = Path::new("fixtures/roda-blog");
    let scratch = scratch_dir("roda-blog");
    generate_ruby_project(fixture, &scratch);

    // The ported oracle rides along into the emitted tree's test/ dir.
    std::fs::create_dir_all(scratch.join("test")).expect("mkdir test");
    std::fs::copy(
        Path::new("tests/roda_oracle/blog_oracle_test.rb"),
        scratch.join("test/blog_oracle_test.rb"),
    )
    .expect("copy ported oracle");

    // Resolve against the emitted tree's own Gemfile (a copy of the
    // scaffold's, which carries rack-test for exactly this gate).
    let gemfile = std::fs::canonicalize(scratch.join("Gemfile")).expect("scratch Gemfile");
    let install = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .arg("install")
        .arg("--quiet")
        .current_dir(&scratch)
        .output()
        .expect("spawn bundle install");
    assert!(
        install.status.success(),
        "bundle install failed\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .arg("exec")
        .arg("ruby")
        .arg("test/blog_oracle_test.rb")
        .current_dir(&scratch)
        .output()
        .expect("spawn ruby");
    assert!(
        output.status.success(),
        "transpiled roda-blog failed its oracle\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
