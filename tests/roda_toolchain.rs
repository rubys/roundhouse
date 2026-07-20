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
//! 2. The step-1 gate (not yet written): ingest `fixtures/roda-blog`
//!    through the Roda/Sequel recognizers, emit the CRuby target, and
//!    run this same oracle suite against the emitted app — the analog
//!    of `real_blog_spinel_tests_pass` in `ruby_toolchain.rs`. It lands
//!    with the ingest front-end (see docs/roda-sequel-plan.md).
//!
//! Like the other toolchain tests this is `#[ignore]`d: it shells out to
//! bundler/MRI, which is a CI (or explicit `--ignored`) concern.

use std::path::Path;
use std::process::Command;

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
