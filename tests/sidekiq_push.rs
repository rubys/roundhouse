//! Sidekiq pushes (`runtime/spinel/sidekiq_push.rb`), held to what real
//! Sidekiq 8.1 sends to Redis for the same jobs.
//!
//! `tests/sidekiq_push/driver.rb` enqueues a fixed set of jobs through
//! `SidekiqPush` with its recording seam on, so no Redis is involved,
//! and prints one line per command. The harness checks them against
//! `tests/sidekiq_push/sidekiq.json`, which `tests/sidekiq_push/
//! oracle.rb` records from Sidekiq 8.1.7's ActiveJob adapter against a
//! real Redis: the same commands and keys, each envelope's keys in the
//! same order on the wire, and the same envelope once the jid, the
//! timestamps and the ActiveJob `job_id` are masked. A ZADD score is
//! compared as its offset from `created_at`.
//!
//! The spinel test compiles the same driver and requires the same
//! output as CRuby, apart from those masked values. Marked `#[ignore]`,
//! as the other spinel tests are; it needs `spinel` on PATH:
//!
//!     cargo test --test sidekiq_push -- --ignored

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
    ("tests/sidekiq_push/driver.rb", "driver.rb"),
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

/// One command the driver recorded.
struct Cmd {
    verb: String,
    key: String,
    score: Option<f64>,
    json: Option<String>,
}

/// The driver's pushes, by name, in order.
fn pushes(stdout: &str) -> Vec<(String, Vec<Cmd>)> {
    let mut out: Vec<(String, Vec<Cmd>)> = Vec::new();
    for line in stdout.lines() {
        if let Some(name) = line.strip_prefix("push ") {
            out.push((name.to_string(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix("cmd ") {
            let mut parts = rest.splitn(4, ' ');
            let verb = parts.next().unwrap().to_string();
            let key = parts.next().unwrap().to_string();
            let score = parts.next().unwrap();
            let json = parts.next().unwrap();
            out.last_mut().expect("a cmd before any push").1.push(Cmd {
                verb,
                key,
                score: (score != "-").then(|| score.parse().expect("a numeric score")),
                json: (json != "-").then(|| json.to_string()),
            });
        }
    }
    out
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

fn is_iso9(v: &Value) -> bool {
    let Some(s) = v.as_str() else { return false };
    s.len() == 30 && s.ends_with('Z') && s.as_bytes()[10] == b'T' && s.as_bytes()[19] == b'.'
}

/// The envelope with the oracle's masks applied, after checking each
/// masked value has Sidekiq's shape. Returns it with `created_at`.
fn mask(raw: &str) -> (Value, i64) {
    let mut env: Value = serde_json::from_str(raw).unwrap_or_else(|e| panic!("not JSON ({e}): {raw}"));
    let created = env["created_at"].as_i64().unwrap_or_else(|| panic!("created_at isn't integer ms: {raw}"));
    assert_eq!(created.to_string().len(), 13, "created_at isn't epoch ms: {raw}");
    let jid = env["jid"].as_str().unwrap();
    assert!(jid.len() == 24 && jid.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "jid: {jid}");
    env["jid"] = "<jid>".into();
    env["created_at"] = "<ms>".into();
    if env.get("enqueued_at").is_some() {
        assert!(env["enqueued_at"].as_i64().is_some(), "enqueued_at isn't integer ms: {raw}");
        env["enqueued_at"] = "<ms>".into();
    }
    for aj in env["args"].as_array_mut().unwrap() {
        let id = aj["job_id"].as_str().unwrap();
        assert!(id.len() == 36 && id.as_bytes()[14] == b'4', "job_id isn't a v4 uuid: {id}");
        aj["job_id"] = "<uuid>".into();
        assert!(is_iso9(&aj["enqueued_at"]), "enqueued_at: {}", aj["enqueued_at"]);
        aj["enqueued_at"] = "<iso8601(9)>".into();
        if !aj["scheduled_at"].is_null() {
            assert!(is_iso9(&aj["scheduled_at"]), "scheduled_at: {}", aj["scheduled_at"]);
            aj["scheduled_at"] = "<iso8601(9)>".into();
        }
    }
    (env, created)
}

#[test]
fn pushes_match_sidekiq() {
    let stdout = run_cruby(&scratch("roundhouse-sidekiq-push-cruby"));
    let failures: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failures.is_empty(), "runtime checks failed:\n{}", failures.join("\n"));

    let oracle: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/sidekiq_push/sidekiq.json")).unwrap(),
    )
    .expect("sidekiq.json is JSON");
    assert_eq!(oracle["sidekiq"], "8.1.7", "the oracle must come from Sidekiq 8.1.7");
    assert_eq!(oracle["activejob"], "8.1.3", "the oracle must come from activejob 8.1.3");

    let got = pushes(&stdout);
    let want = oracle["pushes"].as_array().unwrap();
    assert_eq!(
        got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        want.iter().map(|p| p["name"].as_str().unwrap()).collect::<Vec<_>>(),
        "the driver and the oracle push different jobs"
    );
    for ((name, cmds), w) in got.iter().zip(want) {
        let wcmds: Vec<&Value> = w["round_trips"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| t["commands"].as_array().unwrap())
            .collect();
        assert_eq!(cmds.len(), wcmds.len(), "{name}: command count");
        for (c, wc) in cmds.iter().zip(wcmds) {
            let argv = wc["argv"].as_array().unwrap();
            assert_eq!(c.verb, argv[0].as_str().unwrap(), "{name}: command");
            assert_eq!(c.key, argv[1].as_str().unwrap(), "{name}: key");
            match c.verb.as_str() {
                "sadd" => assert_eq!(c.json.as_deref(), argv[2].as_str(), "{name}: sadd member"),
                "lpush" | "zadd" => {
                    let raw = c.json.as_deref().expect("an envelope");
                    assert_eq!(
                        top_level_keys(raw),
                        wc["envelope_keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect::<Vec<_>>(),
                        "{name}: envelope key order"
                    );
                    let (env, created) = mask(raw);
                    assert_eq!(env, wc["envelope"], "{name}: envelope differs from Sidekiq's");
                    if c.verb == "zadd" {
                        let offset = c.score.unwrap() - created as f64 / 1000.0;
                        let want = wc["score_minus_created_at"].as_f64().unwrap();
                        assert!((offset - want).abs() < 0.5, "{name}: score is {offset}s after created_at, Sidekiq's {want}s");
                    }
                }
                other => panic!("{name}: unexpected command {other}"),
            }
        }
    }
}

/// The real connection (`runtime/spinel/sidekiq/sidekiq_redis.rb`) over
/// a scripted stand-in for the redis gem: a connection error, the
/// first connect's included, takes one reconnect, and a command error
/// none.
#[test]
fn a_connection_error_takes_one_reconnect() {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("roundhouse-sidekiq-push-reconnect");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir scratch");
    for (from, to) in [
        ("runtime/spinel/sidekiq/sidekiq_redis.rb", "sidekiq_redis.rb"),
        ("tests/sidekiq_push/reconnect/redis.rb", "redis.rb"),
        ("tests/sidekiq_push/reconnect/reconnect.rb", "reconnect.rb"),
    ] {
        std::fs::copy(root().join(from), dir.join(to)).expect("copy reconnect source");
    }
    let out = Command::new("ruby")
        .args(["-I.", "reconnect.rb"])
        .current_dir(&dir)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success() && stdout.lines().any(|l| l == "done"),
        "the reconnect driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let failures: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failures.is_empty(), "reconnect checks failed:\n{}", failures.join("\n"));
}

/// The output with every per-run value masked, for the spinel twin.
fn masked_output(stdout: &str) -> String {
    stdout
        .lines()
        .map(|line| match line.strip_prefix("cmd ") {
            Some(rest) => {
                let parts: Vec<&str> = rest.splitn(4, ' ').collect();
                let json = if parts[0] == "sadd" { parts[3].to_string() } else { mask(parts[3]).0.to_string() };
                let score = if parts[2] == "-" { "-" } else { "<score>" };
                format!("cmd {} {} {score} {json}", parts[0], parts[1])
            }
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[ignore]
fn the_spinel_binary_pushes_as_cruby_does() {
    let cruby = run_cruby(&scratch("roundhouse-sidekiq-push-cruby-for-spinel"));
    let dir = scratch("roundhouse-sidekiq-push-spinel");
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
        masked_output(&binary),
        masked_output(&cruby),
        "the binary and CRuby disagree\n=== spinel ===\n{binary}\n=== CRuby ===\n{cruby}"
    );
}
