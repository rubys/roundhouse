//! Sidekiq pushes against a real Redis, read back by real Sidekiq.
//!
//! `tests/sidekiq_push_live/driver.rb` runs the push runtime a tree on
//! `queue_adapter :sidekiq` gets, over its real Redis client: redis-rb
//! under CRuby, and spinel-redis when Spinel compiles it. Then
//! `tests/sidekiq_push_live/reader.rb` reads the queues, the jobs and the
//! scheduled set through Sidekiq's public API (`Sidekiq::Queue`,
//! `Sidekiq::ScheduledSet`), as a Sidekiq process would see them.
//! `tests/sidekiq_push.rs` holds the bytes against Sidekiq's own; this
//! gate proves the clients deliver them and Sidekiq accepts them.
//!
//!   SIDEKIQ_LIVE_REDIS_URL=redis://127.0.0.1:6379/15 \
//!   SPINEL_REDIS_DIR=/path/to/spinel-redis \
//!   cargo test --test sidekiq_push_live -- --ignored --nocapture --test-threads=1
//!
//! One thread: both tests flush and fill the same scratch database.
//! SIDEKIQ_LIVE_REDIS_URL must name a scratch database: each test
//! flushes it. The CRuby test needs the `redis` (~> 5.4) and `sidekiq`
//! (8.1.x) gems; the Spinel test also needs `spinel` and a spinel-redis
//! checkout. Under `--ignored` a missing input fails rather than skips.

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
    ("tests/sidekiq_push_live/driver.rb", "driver.rb"),
    ("tests/sidekiq_push_live/reader.rb", "reader.rb"),
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
    run(Command::new("ruby").args(args).env("REDIS_URL", url).env_remove("REDIS_PROVIDER").current_dir(dir))
}

/// Run the reader's checks over what a push left, and require every one.
fn assert_sidekiq_reads(dir: &Path, url: &str) {
    let out = ruby(dir, url, &["reader.rb", "check"]);
    assert!(out.ends_with("done\n"), "the reader did not finish\n{out}");
    let failed: Vec<&str> = out.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "Sidekiq read the pushes differently:\n{}", failed.join("\n"));
    assert!(out.lines().filter(|l| l.starts_with("ok")).count() >= 20, "{out}");
}

#[test]
#[ignore = "requires SIDEKIQ_LIVE_REDIS_URL (a scratch Redis database) and the redis and sidekiq gems"]
fn cruby_pushes_through_redis_rb_reach_sidekiq() {
    let url = required_env("SIDEKIQ_LIVE_REDIS_URL", "a scratch Redis database");
    let dir = scratch("roundhouse-sidekiq-live-cruby");
    ruby(&dir, &url, &["reader.rb", "flush"]);
    let out = ruby(&dir, &url, &["driver.rb"]);
    assert!(out.contains("pushed to "), "{out}");
    assert_sidekiq_reads(&dir, &url);
}

#[test]
#[ignore = "requires Spinel, SPINEL_REDIS_DIR (a spinel-redis checkout), SIDEKIQ_LIVE_REDIS_URL and the sidekiq gem"]
fn spinel_pushes_through_spinel_redis_reach_sidekiq() {
    let url = required_env("SIDEKIQ_LIVE_REDIS_URL", "a scratch Redis database");
    let redis_dir = required_env("SPINEL_REDIS_DIR", "a spinel-redis checkout");
    assert!(Path::new(&redis_dir).join("redis.rb").exists(), "SPINEL_REDIS_DIR={redis_dir} has no redis.rb");
    let dir = scratch("roundhouse-sidekiq-live-spinel");
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    run(Command::new(compiler).args(["-I", &redis_dir, "driver.rb", "-o", "driver"]).current_dir(&dir));
    ruby(&dir, &url, &["reader.rb", "flush"]);
    let out = run(Command::new("timeout")
        .arg("120")
        .arg(dir.join("driver"))
        .env("REDIS_URL", &url)
        .env_remove("REDIS_PROVIDER")
        .current_dir(&dir));
    assert!(out.contains("pushed to "), "{out}");
    assert_sidekiq_reads(&dir, &url);
}
