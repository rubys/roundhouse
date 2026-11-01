//! The worker's envelope handling (`runtime/spinel/sidekiq_envelope.rb`
//! and `ActiveJob.perform_parsed` in `runtime/spinel/job_registry.rb`),
//! held to what a real Sidekiq 8.1 server writes to Redis.
//!
//! `tests/sidekiq_worker/oracle.rb` runs the real `sidekiq` executable
//! and records `tests/sidekiq_worker/sidekiq.json`: among other things,
//! the jobs it popped in its strict scenario (per-run values pinned)
//! and the dead-set records it wrote for the ones that raised.
//! `tests/sidekiq_worker/driver.rb` feeds the same popped jobs through
//! the runtime and prints the dead record it would write for each
//! failure. They must match Sidekiq's key for key, in the same order,
//! once the timestamps are masked. The driver's own checks cover the
//! readers, the outcomes and the in-process drain.
//!
//! `tests/sidekiq_worker/worker_driver.rb` runs the worker role
//! (`runtime/spinel/sidekiq_worker.rb`) against a scripted
//! `SidekiqRedis`: option parsing, fetch order, a processor running the
//! same popped jobs, and shutdown with a job still running. Its dead
//! records must match Sidekiq's too.
//!
//! `tests/sidekiq_worker/retry_oracle.rb` records
//! `tests/sidekiq_worker/sidekiq_retry.json`: jobs real Sidekiq failed
//! under each of its retry settings, as popped, and what it wrote for
//! each (the retry or dead record, or nothing). The worker driver runs
//! the same failures through the worker's retry rules, which must write
//! the same thing, and the jobs Sidekiq's poller moved back to their
//! queues through the worker's scheduler, which must push the same.
//!
//! The spinel tests compile the same drivers and require the same
//! output as CRuby. Marked `#[ignore]`, as the other spinel tests are;
//! it needs `spinel` on PATH:
//!
//!     cargo test --test sidekiq_worker -- --ignored

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const SOURCES: &[(&str, &str)] = &[
    ("runtime/spinel/base64.rb", "base64.rb"),
    ("runtime/ruby/active_job.rb", "active_job.rb"),
    ("runtime/spinel/global_id_locator.rb", "global_id_locator.rb"),
    ("runtime/spinel/active_job_serialization.rb", "active_job_serialization.rb"),
    ("runtime/spinel/sidekiq_redis.rb", "sidekiq_redis.rb"),
    ("runtime/spinel/sidekiq_push.rb", "sidekiq_push.rb"),
    ("runtime/spinel/job_registry.rb", "job_registry.rb"),
    ("runtime/spinel/sidekiq_envelope.rb", "sidekiq_envelope.rb"),
    ("runtime/spinel/sidekiq/sidekiq_worker.rb", "sidekiq_worker.rb"),
    ("tests/sidekiq_worker/driver.rb", "driver.rb"),
    ("tests/sidekiq_worker/worker_driver.rb", "worker_driver.rb"),
];

/// The driver's clock (`NOW_MS` in driver.rb).
const NOW_MS: i64 = 1_792_632_731_000;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn oracle() -> Value {
    let oracle: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/sidekiq_worker/sidekiq.json")).unwrap(),
    )
    .expect("sidekiq.json is JSON");
    assert_eq!(oracle["versions"]["sidekiq"], "8.1.7", "the oracle must come from Sidekiq 8.1.7");
    oracle
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
    let oracle = oracle();
    let popped: Vec<&str> = oracle["strict"]["popped_from_high_pinned"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    std::fs::write(dir.join("popped.txt"), popped.join("\n") + "\n").expect("write popped.txt");
    let lines: Vec<String> = retry_failures(&retry_oracle())
        .iter()
        .map(|f| {
            format!(
                "{}\t{}\t{}\t{}",
                f["queue"].as_str().unwrap(),
                f["error_class"].as_str().unwrap(),
                f["error_message"].as_str().unwrap(),
                unpin_failure_times(f["popped"].as_str().unwrap())
            )
        })
        .collect();
    std::fs::write(dir.join("failures.txt"), lines.join("\n") + "\n").expect("write failures.txt");
    let moves: Vec<String> = retry_moves(&retry_oracle())
        .iter()
        .map(|m| format!("{}\t{}", m["set"].as_str().unwrap(), m["popped"].as_str().unwrap()))
        .collect();
    std::fs::write(dir.join("moves.txt"), moves.join("\n") + "\n").expect("write moves.txt");
    dir
}

/// The retry oracle's moves (each job Sidekiq's poller popped and pushed
/// back), scenario by scenario.
fn retry_moves(oracle: &Value) -> Vec<&Value> {
    oracle["moves"].as_object().unwrap().values().flat_map(|v| v.as_array().unwrap()).collect()
}

fn retry_oracle() -> Value {
    let oracle: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/sidekiq_worker/sidekiq_retry.json")).unwrap(),
    )
    .expect("sidekiq_retry.json is JSON");
    assert_eq!(oracle["versions"]["sidekiq"], "8.1.7", "the retry oracle must come from Sidekiq 8.1.7");
    oracle
}

/// The retry oracle's failures, scenario by scenario, in its order.
fn retry_failures(oracle: &Value) -> Vec<&Value> {
    oracle["failures"].as_object().unwrap().values().flat_map(|v| v.as_array().unwrap()).collect()
}

/// The oracle pins a failed job's `failed_at` and `retried_at` (see
/// `PINNED` in oracle_support.rb). The worker reads `failed_at` for
/// `retry_for`, so they become a few seconds ago, as they were when
/// Sidekiq failed the job again.
fn unpin_failure_times(raw: &str) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    raw.replace("1792632725000", &(now - 5_000).to_string())
        .replace("1792632729000", &(now - 1_000).to_string())
}

fn run_cruby(dir: &Path, driver: &str) -> String {
    let out = Command::new("ruby")
        .arg(driver)
        .current_dir(dir)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success() && stdout.lines().any(|l| l == "done"),
        "the CRuby driver {driver} did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// The top-level keys of a JSON object, in the order they appear in the
/// text (serde_json's map sorts them).
fn top_level_keys(raw: &str) -> Vec<String> {
    let bytes = raw.as_bytes();
    let mut keys = Vec::new();
    let mut depth = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let mut j = i + 1;
                while bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                if depth == 1 && bytes.get(j + 1) == Some(&b':') {
                    keys.push(serde_json::from_str::<String>(&raw[i..=j]).unwrap());
                }
                i = j;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    keys
}

/// A dead record the driver wrote, with the values the oracle masks
/// masked the same way: the pinned ones the job came with, and the
/// driver's clock where the rewrite wrote it.
fn mask(raw: &str) -> Value {
    let mut env: Value = serde_json::from_str(raw).unwrap_or_else(|e| panic!("not JSON ({e}): {raw}"));
    env["jid"] = "<jid>".into();
    env["created_at"] = "<ms>".into();
    env["enqueued_at"] = "<ms>".into();
    for key in ["failed_at", "retried_at"] {
        if env.get(key).and_then(Value::as_i64) == Some(NOW_MS) {
            env[key] = "<ms>".into();
        }
    }
    for aj in env["args"].as_array_mut().unwrap() {
        aj["job_id"] = "<uuid>".into();
        aj["enqueued_at"] = "<iso8601(9)>".into();
    }
    env
}

/// The dead records Sidekiq wrote in the oracle's strict scenario.
fn sidekiq_dead_records(oracle: &Value) -> Vec<&Value> {
    oracle["strict"]["round_trips"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["commands"].as_array().unwrap())
        .filter(|c| c["argv"][0] == "zadd" && c.get("envelope").is_some())
        .collect()
}

fn assert_no_failures(stdout: &str) {
    let failures: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failures.is_empty(), "runtime checks failed:\n{}", failures.join("\n"));
}

fn assert_matches_sidekiq(got: &[String], want: &[&Value]) {
    assert_eq!(got.len(), want.len(), "dead records written: {got:?}");
    for (raw, w) in got.iter().zip(want) {
        assert_eq!(
            top_level_keys(raw),
            w["envelope_keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect::<Vec<_>>(),
            "dead record key order differs from Sidekiq's: {raw}"
        );
        assert_eq!(mask(raw), w["envelope"], "dead record differs from Sidekiq's: {raw}");
    }
}

#[test]
fn dead_records_match_sidekiq() {
    let stdout = run_cruby(&scratch("roundhouse-sidekiq-worker-cruby"), "driver.rb");
    assert_no_failures(&stdout);
    let oracle = oracle();
    let got: Vec<String> = stdout.lines().filter_map(|l| l.strip_prefix("dead ")).map(str::to_string).collect();
    assert_matches_sidekiq(&got, &sidekiq_dead_records(&oracle));
}

/// The worker role's dead writes: Sidekiq's records for the ActiveJob
/// failures, the malformed job as it was popped, and the trims Sidekiq
/// sends (scores 15552000 s old and older, ranks up to -10000).
#[test]
fn the_worker_runs_and_shuts_down_as_sidekiq_does() {
    let stdout = run_cruby(&scratch("roundhouse-sidekiq-worker-role-cruby"), "worker_driver.rb");
    assert_no_failures(&stdout);
    let oracle = oracle();
    let mut active_job = Vec::new();
    let mut other = Vec::new();
    for line in stdout.lines().filter_map(|l| l.strip_prefix("dead ")) {
        let (record, trims) = line.rsplit_once(" trim ").expect("a dead line names its trims");
        assert_eq!(trims, "age 15552000 keep -10000", "{line}");
        if record.contains("\"Sidekiq::ActiveJob::Wrapper\",\"jid\"") {
            active_job.push(record.replace("\"<now>\"", &NOW_MS.to_string()));
        } else {
            other.push(record.to_string());
        }
    }
    assert_matches_sidekiq(&active_job, &sidekiq_dead_records(&oracle));
    let malformed = oracle["strict"]["round_trips"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["commands"].as_array().unwrap())
        .find_map(|c| c.get("raw"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(other.first().map(String::as_str), Some(malformed), "malformed JSON goes to dead as it is");
    assert!(other.iter().any(|r| r == "null"), "a non-object envelope goes to dead as it is: {other:?}");
    assert_eq!(other.len(), 2, "{other:?}");
    // The jobs with retries left go to the retry set; the retry: false
    // one is dropped.
    let retried: Vec<&str> = stdout.lines().filter_map(|l| l.strip_prefix("retry delay ok ")).collect();
    assert!(
        retried.iter().any(|r| r.contains("\"wrapped\":\"NoArgsJob\"") && r.contains("\"error_class\":\"NoMethodError\"")),
        "an envelope without args fails its job: {retried:?}"
    );
    assert!(
        retried.iter().any(|r| r.contains("\"class\":\"HardWorker\"") && r.contains("\"error_class\":\"NameError\"")),
        "a native Sidekiq job fails as an unknown class: {retried:?}"
    );
    assert_eq!(retried.len(), 2, "{retried:?}");
}

/// Sidekiq's retry rules: for each job the retry oracle's Sidekiq
/// failed, the worker writes what Sidekiq wrote (a retry record due
/// after Sidekiq's backoff, a dead record with its trims, or nothing),
/// key for key and in the same order.
#[test]
fn the_worker_retries_as_sidekiq_does() {
    let stdout = run_cruby(&scratch("roundhouse-sidekiq-worker-retry-cruby"), "worker_driver.rb");
    assert_no_failures(&stdout);
    let oracle = retry_oracle();
    let failures = retry_failures(&oracle);
    let rules: Vec<&str> = stdout.lines().filter_map(|l| l.strip_prefix("rule ")).collect();
    assert_eq!(rules.len(), failures.len(), "{rules:#?}");
    for (rule, want) in rules.iter().zip(&failures) {
        let wrote = want["wrote"].as_str().unwrap();
        let record = match wrote {
            "nothing" => {
                assert_eq!(*rule, "nothing", "Sidekiq wrote nothing for {}", want["popped"]);
                continue;
            }
            "retry" => rule.strip_prefix("retry delay ok ").unwrap_or_else(|| panic!("want a retry write: {rule}")),
            "dead" => {
                let line = rule.strip_prefix("dead ").unwrap_or_else(|| panic!("want a dead write: {rule}"));
                let (record, trims) = line.rsplit_once(" trim ").unwrap();
                assert_eq!(trims, "age 15552000 keep -10000", "{rule}");
                record
            }
            other => panic!("unknown write {other}"),
        };
        assert_eq!(
            top_level_keys(record),
            want["record_keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect::<Vec<_>>(),
            "{wrote} record key order differs from Sidekiq's: {record}"
        );
        let mut got = mask(record);
        for key in ["failed_at", "retried_at"] {
            if got.get(key).and_then(Value::as_str) == Some("<now>") {
                got[key] = "<ms>".into();
            }
        }
        assert_eq!(got, want["record"], "{wrote} record differs from Sidekiq's: {record}");
    }
}

fn spinel_twin(name: &str, driver: &str) {
    let cruby = run_cruby(&scratch(&format!("{name}-cruby-for-spinel")), driver);
    let dir = scratch(&format!("{name}-spinel"));
    let build = Command::new("spinel")
        .args([driver, "-o", "driver"])
        .current_dir(&dir)
        .output()
        .expect("spinel is on PATH");
    assert!(
        build.status.success(),
        "spinel failed to compile {driver}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(dir.join("driver")).current_dir(&dir).output().unwrap();
    let binary = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(binary, cruby, "the binary and CRuby disagree\n=== spinel ===\n{binary}\n=== CRuby ===\n{cruby}");
}

#[test]
#[ignore]
fn the_spinel_binary_writes_what_cruby_writes() {
    spinel_twin("roundhouse-sidekiq-worker", "driver.rb");
}

#[test]
#[ignore]
fn the_spinel_worker_runs_as_cruby_does() {
    spinel_twin("roundhouse-sidekiq-worker-role", "worker_driver.rb");
}

/// Sidekiq's poller: one cycle moves every due job from `retry`, then
/// from `schedule`, and pushes each onto the queue Sidekiq pushed it to,
/// with the same text but for `enqueued_at` (set in place for a retry,
/// appended for a scheduled job).
#[test]
fn the_scheduler_moves_due_jobs_as_sidekiq_does() {
    let stdout = run_cruby(&scratch("roundhouse-sidekiq-worker-scheduler-cruby"), "worker_driver.rb");
    assert_no_failures(&stdout);
    let oracle = retry_oracle();
    let mut want = retry_moves(&oracle);
    // Retry first, then schedule, each in the order they were due.
    want.sort_by_key(|m| m["set"].as_str() != Some("retry"));
    let lines: Vec<&str> = stdout.lines().filter_map(|l| l.strip_prefix("sched ")).collect();
    assert_eq!(lines.first().copied(), Some("script load zpopbyscore"), "{lines:#?}");
    let pushes: Vec<&str> = lines[1..].iter().map(|l| l.strip_prefix("push ").unwrap_or_else(|| panic!("{l}"))).collect();
    assert_eq!(pushes.len(), want.len(), "{pushes:#?}");
    for (push, w) in pushes.iter().zip(&want) {
        let (queue, record) = push.split_once(' ').unwrap();
        assert_eq!(queue, w["queue"].as_str().unwrap(), "{push}");
        assert_eq!(
            top_level_keys(record),
            w["pushed_keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect::<Vec<_>>(),
            "pushed key order differs from Sidekiq's: {record}"
        );
        let mut got = mask(record);
        for key in ["failed_at", "retried_at"] {
            if matches!(got.get(key).and_then(Value::as_i64), Some(1_792_632_725_000 | 1_792_632_729_000)) {
                got[key] = "<ms>".into();
            }
        }
        for aj in got["args"].as_array_mut().unwrap() {
            if !aj["scheduled_at"].is_null() {
                aj["scheduled_at"] = "<iso8601(9)>".into();
            }
        }
        assert_eq!(got, w["pushed"], "pushed job differs from Sidekiq's: {record}");
    }
}
