//! ActiveJob payloads (`runtime/spinel/active_job_serialization.rb`),
//! held to Rails' own output and read back by the runtime's readers.
//!
//! `tests/active_job_payload/driver.rb` writes a fixed set of arguments
//! through `ActiveJob::Arguments`, builds a payload with
//! `ActiveJob::Payload.build`, reads every argument back, and runs Procs
//! and payloads through one queue. The harness checks its output against
//! `tests/active_job_payload/rails.json`, which
//! `tests/active_job_payload/oracle.rb` prints from real activejob 8.1.3:
//! the arguments must be the same JSON, and the payload must have Rails'
//! keys and values, apart from `job_id` and `enqueued_at`.
//!
//! The spinel test runs the same driver compiled by spinel and requires
//! the same output as CRuby. Marked `#[ignore]`, as the other spinel
//! tests are; it needs `spinel` on PATH:
//!
//!     cargo test --test active_job_payload -- --ignored

use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCES: &[(&str, &str)] = &[
    ("runtime/spinel/base64.rb", "base64.rb"),
    ("runtime/ruby/active_job.rb", "active_job.rb"),
    ("runtime/spinel/global_id_locator.rb", "global_id_locator.rb"),
    ("runtime/spinel/active_job_serialization.rb", "active_job_serialization.rb"),
    ("tests/active_job_payload/driver.rb", "driver.rb"),
];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir scratch");
    for (from, to) in SOURCES {
        std::fs::copy(root().join(from), dir.join(to)).expect("copy runtime source");
    }
    dir
}

fn run_cruby(dir: &Path) -> String {
    let out = Command::new("ruby")
        .arg("driver.rb")
        .current_dir(dir)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success() && stdout.lines().any(|l| l == "done"),
        "the CRuby driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

fn line<'a>(stdout: &'a str, tag: &str) -> &'a str {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(tag))
        .unwrap_or_else(|| panic!("no `{tag}` line\n{stdout}"))
}

fn parse(text: &str, what: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{what} is not JSON ({e}): {text}"))
}

#[test]
fn payloads_match_rails_and_read_back() {
    let stdout = run_cruby(&scratch("roundhouse-active-job-payload-cruby"));
    let failures: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failures.is_empty(), "read-side checks failed:\n{}", failures.join("\n"));

    let rails = parse(
        &std::fs::read_to_string(root().join("tests/active_job_payload/rails.json")).unwrap(),
        "rails.json",
    );
    assert_eq!(rails["activejob"], "8.1.3", "the oracle must come from activejob 8.1.3");

    let args = parse(line(&stdout, "args "), "the arguments");
    let expected = rails["arguments"].as_array().unwrap();
    let got = args.as_array().unwrap();
    assert_eq!(got.len(), expected.len(), "argument count");
    for (i, (g, e)) in got.iter().zip(expected).enumerate() {
        assert_eq!(g, e, "argument {i} differs from Rails'");
    }

    let mut job = parse(line(&stdout, "job "), "the payload");
    let mut rails_job = rails["job"].clone();
    for key in ["job_id", "enqueued_at"] {
        assert!(job.get(key).is_some() && rails_job.get(key).is_some(), "`{key}` missing");
        job[key] = serde_json::Value::Null;
        rails_job[key] = serde_json::Value::Null;
    }
    let keys = |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&job), keys(&rails_job), "the payload's keys differ from Rails'");
    assert_eq!(job, rails_job, "the payload differs from Rails' job.serialize");
}

#[test]
#[ignore]
fn the_spinel_binary_writes_and_reads_as_cruby_does() {
    let cruby = run_cruby(&scratch("roundhouse-active-job-payload-cruby-for-spinel"));
    let dir = scratch("roundhouse-active-job-payload-spinel");
    let build = Command::new("spinel")
        .args(["driver.rb", "-o", "driver"])
        .current_dir(&dir)
        .output()
        .expect("spinel is on PATH");
    assert!(
        build.status.success(),
        "spinel failed to compile the driver\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(dir.join("driver")).current_dir(&dir).output().unwrap();
    let binary = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(
        binary, cruby,
        "the binary and CRuby disagree\n=== spinel ===\n{binary}\n=== CRuby ===\n{cruby}"
    );
}
