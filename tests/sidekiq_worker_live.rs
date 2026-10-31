//! The worker role against a real Redis, read back by real Sidekiq.
//!
//! `tests/sidekiq_worker_live/driver.rb` runs the worker a tree on
//! `queue_adapter :sidekiq` gets (`./app sidekiq`), over its real Redis
//! client: redis-rb under CRuby, and spinel-redis when Spinel compiles
//! it. It pushes jobs as the web role does, lets two processor threads
//! run them and the scheduler move one pushed for a second later, then
//! shuts down with a job still running.
//! `tests/sidekiq_worker_live/reader.rb` reads what is left through
//! Sidekiq's public API (`Sidekiq::RetrySet`, `Sidekiq::Queue`): the two
//! failed jobs in the retry set with Sidekiq's error fields, and the
//! running job back on its queue, unchanged. `tests/sidekiq_worker.rs`
//! holds the bytes against Sidekiq's own; this gate proves the clients
//! deliver them and Sidekiq reads them.
//!
//!   SIDEKIQ_LIVE_REDIS_URL=redis://127.0.0.1:6379/15 \
//!   SPINEL_REDIS_DIR=/path/to/spinel-redis \
//!   cargo test --test sidekiq_worker_live -- --ignored --nocapture --test-threads=1
//!
//! The shutdown tests run `tests/sidekiq_worker_live/worker.rb` as its
//! own process, the way `./app sidekiq` runs, with jobs pushed by real
//! Rails (`rails_push.rb`, activejob through Sidekiq's adapter). They
//! signal it from outside: `TSTP` stops it fetching, `TERM` waits `-t 1`
//! and pushes the running job back, and the process exits 0. A second
//! worker then runs the requeued job and the one queued while the first
//! was quiet, so no job is lost.
//!
//! One thread: the tests flush and fill the same scratch database, as
//! `sidekiq_push_live` does. The Spinel tests need a spinel-redis with
//! `brpop` (v0.3.1) and, for the scheduler, `script` and `evalsha` (v0.3.2).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

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
    ("tests/sidekiq_worker_live/stubs.rb", "stubs.rb"),
    ("tests/sidekiq_worker_live/driver.rb", "driver.rb"),
    ("tests/sidekiq_worker_live/worker.rb", "worker.rb"),
    ("tests/sidekiq_worker_live/rails_push.rb", "rails_push.rb"),
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
    assert!(out.contains("scheduled ran, 0 left, 2 to retry\n"), "{out}");
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
#[ignore = "requires Spinel, SPINEL_REDIS_DIR (spinel-redis with scripting), SIDEKIQ_LIVE_REDIS_URL and the sidekiq gem"]
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

/// A worker process, its stdout read line by line on a thread so the
/// test can wait for what it prints. Killed on drop, so a failed check
/// leaves no worker behind.
struct Worker {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Worker {
    fn start(mut command: Command, url: &str, slow_seconds: &str) -> Worker {
        let mut child = command
            .env("REDIS_URL", url)
            .env("SLOW_SECONDS", slow_seconds)
            .env_remove("REDIS_PROVIDER")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("{command:?}: {e}"));
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Worker { child, lines, seen: Vec::new() }
    }

    /// Wait up to `seconds` for a line containing `text`.
    fn wait_for(&mut self, text: &str, seconds: u64) {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while !self.seen.iter().any(|l| l.contains(text)) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    println!("worker: {line}");
                    self.seen.push(line);
                }
                Err(RecvTimeoutError::Timeout) => panic!("no {text:?} within {seconds} s; the worker printed:\n{}", self.seen.join("\n")),
                Err(RecvTimeoutError::Disconnected) => panic!("the worker exited before {text:?}:\n{}", self.seen.join("\n")),
            }
        }
    }

    fn signal(&self, name: &str) {
        let status = Command::new("kill").arg(format!("-{name}")).arg(self.child.id().to_string()).status().unwrap();
        assert!(status.success(), "kill -{name}");
    }

    /// TERM, then the exit: 0, within the `-t` deadline and the idle
    /// fetchers' wait. Answers everything the worker printed.
    fn term_and_wait(mut self) -> String {
        self.signal("TERM");
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "the worker did not exit within 15 s of TERM:\n{}", self.seen.join("\n"));
            std::thread::sleep(Duration::from_millis(50));
        };
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(2)) {
            println!("worker: {line}");
            self.seen.push(line);
        }
        let out = self.seen.join("\n");
        assert!(status.success(), "the worker exited {status} after TERM:\n{out}");
        out
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The shutdown round, with `worker` building the command that starts
/// one worker process (`ruby worker.rb sidekiq ...` or the Spinel
/// binary).
fn shutdown_round(dir: &Path, url: &str, worker: &dyn Fn() -> Command) {
    // The last line: Sidekiq logs its connect to stdout first.
    let sizes = || ruby(dir, url, &["reader.rb", "sizes"]).lines().last().unwrap_or("").to_string();
    ruby(dir, url, &["reader.rb", "flush"]);
    ruby(dir, url, &["rails_push.rb", "RecordJob:rails1"]);

    let mut first = Worker::start(worker(), url, "10");
    first.wait_for("ran rails1", 20);
    ruby(dir, url, &["rails_push.rb", "SlowJob:s1"]);
    first.wait_for("slow started s1", 20);

    // Quiet: nothing more is fetched, and a job a parked fetch takes
    // meanwhile goes straight back.
    first.signal("TSTP");
    first.wait_for("no longer accepting new work", 10);
    ruby(dir, url, &["rails_push.rb", "RecordJob:q1"]);
    std::thread::sleep(Duration::from_secs(3));

    let out = first.term_and_wait();
    assert!(!out.contains("ran q1"), "a quiet worker ran a job:\n{out}");
    assert!(!out.contains("slow done s1"), "the slow job finished before TERM's deadline:\n{out}");
    assert!(out.contains("Pushed 1 jobs back to Redis"), "{out}");
    assert!(out.contains("Bye!"), "{out}");
    assert_eq!(sizes(), "default=2 dead=0 classes=RecordJob,SlowJob");

    // A second worker runs both: none was lost.
    let mut second = Worker::start(worker(), url, "0.2");
    second.wait_for("ran q1", 20);
    second.wait_for("slow done s1", 20);
    let out = second.term_and_wait();
    assert!(out.contains("Bye!") && !out.contains("jobs back"), "{out}");
    assert_eq!(sizes(), "default=0 dead=0 classes=");
}

#[test]
#[ignore = "requires SIDEKIQ_LIVE_REDIS_URL and the redis, sidekiq and activejob gems"]
fn the_cruby_worker_process_shuts_down_on_term_and_loses_no_job() {
    let url = required_env("SIDEKIQ_LIVE_REDIS_URL", "a scratch Redis database");
    let dir = scratch("roundhouse-sidekiq-worker-shutdown-cruby");
    let worker = || {
        let mut c = Command::new("ruby");
        c.args(["worker.rb", "sidekiq", "-q", "default", "-c", "2", "-t", "1"]).current_dir(&dir);
        c
    };
    shutdown_round(&dir, &url, &worker);
}

#[test]
#[ignore = "requires Spinel, SPINEL_REDIS_DIR (spinel-redis with scripting), SIDEKIQ_LIVE_REDIS_URL and the sidekiq and activejob gems"]
fn the_spinel_worker_process_shuts_down_on_term_and_loses_no_job() {
    let url = required_env("SIDEKIQ_LIVE_REDIS_URL", "a scratch Redis database");
    let redis_dir = required_env("SPINEL_REDIS_DIR", "a spinel-redis checkout");
    let dir = scratch("roundhouse-sidekiq-worker-shutdown-spinel");
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    run(Command::new(compiler).args(["-I", &redis_dir, "worker.rb", "-o", "worker"]).current_dir(&dir));
    let worker = || {
        let mut c = Command::new(dir.join("worker"));
        c.args(["sidekiq", "-q", "default", "-c", "2", "-t", "1"]).current_dir(&dir);
        c
    };
    shutdown_round(&dir, &url, &worker);
}
