//! The emitted falcon.rb front (runtime/spinel/scaffold/ruby_overlay):
//! config.ru is loaded ONCE, in the parent, before the workers fork, so
//! the app's code and boot heap are shared copy-on-write; the workers
//! serve; ROUNDHOUSE_PRELOAD=0 brings back the load per child.
//!
//! Needs the falcon gem, which the emitted Gemfile does not name (the
//! bench's ruby-falcon lane adds it), so it is not run by default.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CONFIG_RU: &str = r#"
require "rack"
require_relative "runtime/thruster_cache"
File.open(ENV.fetch("LOAD_LOG"), "a") { |f| f.puts Process.pid }
run ->(env) { [200, { "content-type" => "text/plain" }, [Process.pid.to_s]] }
"#;

fn scratch_dir(tag: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("roundhouse-falcon-front").join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("runtime")).unwrap();
    dir
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn get(port: u16) -> Option<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    s.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    out.split("\r\n\r\n").nth(1).map(|b| b.trim().to_string())
}

/// Boots falcon.rb with two workers; returns (server pid, pids that
/// loaded config.ru, pids that answered).
fn boot(preload: bool) -> (u32, Vec<String>, Vec<String>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let overlay = root.join("runtime/spinel/scaffold/ruby_overlay");
    let dir = scratch_dir(if preload { "preload" } else { "per-child" });
    std::fs::copy(overlay.join("falcon.rb"), dir.join("falcon.rb")).unwrap();
    for file in ["runtime/thruster_cache.rb", "runtime/cable_hub.rb"] {
        std::fs::copy(overlay.join(file), dir.join(file)).unwrap();
    }
    std::fs::write(dir.join("config.ru"), CONFIG_RU).unwrap();
    let log = dir.join("loads.txt");
    let port = free_port();
    let mut child = Command::new("ruby")
        .arg("falcon.rb")
        .current_dir(&dir)
        .env("PORT", port.to_string())
        .env("BIND", "127.0.0.1")
        .env("WEB_CONCURRENCY", "2")
        .env("ROUNDHOUSE_CHECKPOINTER", "0")
        .env("ROUNDHOUSE_PRELOAD", if preload { "1" } else { "0" })
        .env("LOAD_LOG", &log)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("ruby is on PATH");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut answered = Vec::new();
    while Instant::now() < deadline && answered.is_empty() {
        if let Some(b) = get(port) {
            answered.push(b);
        } else {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    for _ in 0..40 {
        if let Some(b) = get(port) {
            answered.push(b);
        }
    }
    let pid = child.id();
    let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    let _ = child.wait();
    let loads = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    (pid, loads, answered)
}

#[test]
#[ignore = "requires the falcon gem"]
fn the_falcon_front_loads_config_ru_once_in_the_parent() {
    let (pid, loads, answered) = boot(true);
    assert!(!answered.is_empty(), "falcon.rb never answered");
    assert_eq!(loads, vec![pid.to_string()], "config.ru should load once, in the parent");
    assert!(
        answered.iter().all(|p| *p != pid.to_string()),
        "the parent served a request: {answered:?}"
    );

    let (pid, loads, answered) = boot(false);
    assert!(!answered.is_empty(), "falcon.rb (ROUNDHOUSE_PRELOAD=0) never answered");
    assert_eq!(loads.len(), 2, "one load per worker without preload: {loads:?}");
    assert!(!loads.contains(&pid.to_string()), "the parent loaded config.ru: {loads:?}");
}
