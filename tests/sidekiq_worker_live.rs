//! The worker role against a real Redis, read back by real Sidekiq.
//!
//! `tests/sidekiq_worker_live/driver.rb` runs the worker a tree on
//! `queue_adapter :sidekiq` gets (`./app sidekiq`), over its real Redis
//! client: redis-rb under CRuby, and spinel-redis when Spinel compiles
//! it. It pushes jobs as the web role does, lets two processor threads
//! run them, then shuts down with a job still running.
//! `tests/sidekiq_worker_live/reader.rb` reads what is left through
//! Sidekiq's public API (`Sidekiq::DeadSet`, `Sidekiq::Queue`): the two
//! failed jobs in the dead set with Sidekiq's error fields, and the
//! running job back on its queue, unchanged. `tests/sidekiq_worker.rs`
//! holds the bytes against Sidekiq's own; this gate proves the clients
//! deliver them and Sidekiq reads them.
//!
//!   SIDEKIQ_LIVE_REDIS_URL=redis://127.0.0.1:6379/15 \
//!   SPINEL_REDIS_DIR=/path/to/spinel-redis \
//!   cargo test --test sidekiq_worker_live -- --ignored --nocapture --test-threads=1
//!
//! One thread: both tests flush and fill the same scratch database, as
//! `sidekiq_push_live` does. The Spinel test needs spinel-redis v0.3.1
//! or later, for `brpop`.

use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCES: &[(&str, &str)] = &[
    ("runtime/spinel/base64.rb", "base64.rb"),
    ("runtime/ruby/active_job.rb", "active_job.rb"),
    ("runtime/spinel/global_id_locator.rb", "global_id_locator.rb"),
    ("runtime/spinel/active_job_serialization.rb", "active_job_serialization.rb"),
    ("runtime/spinel/sidekiq/sidekiq_redis.rb", "sidekiq_redis.rb"),
    ("runtime/spinel/sidekiq_push.rb", "sidekiq_push.rb"),
    ("runtime/spinel/job_registry.rb", "job_registry.rb"),
    ("runtime/spinel/sidekiq_envelope.rb", "sidekiq_envelope.rb"),
    ("runtime/spinel/sidekiq/sidekiq_worker.rb", "sidekiq_worker.rb"),
    ("tests/sidekiq_worker_live/driver.rb", "driver.rb"),
    ("tests/sidekiq_worker_live/reader.rb", "reader.rb"),
];

fn required_env(name: &str, what: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v,
        _ => panic!("{name} must name {what}; this gate fails rather than skips without it"),
    }
}

fn scratch(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let base = option_env!("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir scratch");
    for (from, to) in SOURCES {
        std::fs::copy(root.join(from), dir.join(to)).unwrap_or_else(|e| panic!("copy {from}: {e}"));
    }
    dir
}

fn run(command: &mut Command) -> String {
    let out = command.output().unwrap_or_else(|e| panic!("{command:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{command:?}: {}\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    print!("{stdout}");
    stdout
}

fn ruby(dir: &Path, url: &str, args: &[&str]) -> String {
    run(Command::new("timeout").arg("120").arg("ruby").args(args).env("REDIS_URL", url).env_remove("REDIS_PROVIDER").current_dir(dir))
}

/// What the driver says it did: the three jobs that ran, and the slow
/// one pushed back.
fn assert_worked(out: &str) {
    assert!(out.contains("ran r1,r2,r3\n"), "{out}");
    assert!(out.contains("requeued, 1 on default\n"), "{out}");
    assert!(out.contains("worked "), "{out}");
}

/// Run the reader's checks over what the worker left, and require every one.
fn assert_sidekiq_reads(dir: &Path, url: &str) {
    let out = ruby(dir, url, &["reader.rb", "check"]);
    assert!(out.ends_with("done\n"), "the reader did not finish\n{out}");
    let failed: Vec<&str> = out.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "Sidekiq read the worker's writes differently:\n{}", failed.join("\n"));
    assert!(out.lines().filter(|l| l.starts_with("ok")).count() >= 17, "{out}");
}

#[test]
#[ignore = "requires SIDEKIQ_LIVE_REDIS_URL (a scratch Redis database) and the redis and sidekiq gems"]
fn the_cruby_worker_runs_jobs_sidekiq_pushed_and_reads() {
    let url = required_env("SIDEKIQ_LIVE_REDIS_URL", "a scratch Redis database");
    let dir = scratch("roundhouse-sidekiq-worker-live-cruby");
    ruby(&dir, &url, &["reader.rb", "flush"]);
    assert_worked(&ruby(&dir, &url, &["driver.rb"]));
    assert_sidekiq_reads(&dir, &url);
}

#[test]
#[ignore = "requires Spinel, SPINEL_REDIS_DIR (spinel-redis v0.3.1+), SIDEKIQ_LIVE_REDIS_URL and the sidekiq gem"]
fn the_spinel_worker_runs_jobs_over_spinel_redis() {
    let url = required_env("SIDEKIQ_LIVE_REDIS_URL", "a scratch Redis database");
    let redis_dir = required_env("SPINEL_REDIS_DIR", "a spinel-redis checkout");
    assert!(Path::new(&redis_dir).join("redis.rb").exists(), "SPINEL_REDIS_DIR={redis_dir} has no redis.rb");
    let dir = scratch("roundhouse-sidekiq-worker-live-spinel");
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    run(Command::new(compiler).args(["-I", &redis_dir, "driver.rb", "-o", "driver"]).current_dir(&dir));
    ruby(&dir, &url, &["reader.rb", "flush"]);
    let out = run(Command::new("timeout")
        .arg("120")
        .arg(dir.join("driver"))
        .env("REDIS_URL", &url)
        .env_remove("REDIS_PROVIDER")
        .current_dir(&dir));
    assert_worked(&out);
    assert_sidekiq_reads(&dir, &url);
}
