//! `scripts/bench-paired`: the per-route paired benchmark.
//!
//! The statistics are checked through `--analyze`, which re-reads a
//! result file, so they run against hand-written samples with known
//! answers. The measurement loop is checked end to end against a stub
//! `wrk` with fixed rates and a local server that answers the preflight,
//! so no timing noise reaches an assertion.

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/bench-paired")
}

fn scratch(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "bench-paired-{name}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new("ruby")
        .arg(script())
        .args(args)
        .output()
        .expect("ruby is on PATH")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn row<'a>(table: &'a str, path: &str) -> &'a str {
    table
        .lines()
        .find(|l| l.split_whitespace().next() == Some(path))
        .unwrap_or_else(|| panic!("no row for {path} in\n{table}"))
}

/// One route whose pairs have the given target/reference ratios, around a
/// 1000µs reference.
fn route(path: &str, ratios: &[f64]) -> serde_json::Value {
    let pairs: Vec<_> = ratios
        .iter()
        .enumerate()
        .map(|(i, r)| {
            serde_json::json!({
                "order": if i % 2 == 0 { "reference-first" } else { "target-first" },
                "reference_us": 1000.0,
                "target_us": 1000.0 * r,
            })
        })
        .collect();
    serde_json::json!({ "path": path, "attempts": [{ "pairs": pairs }] })
}

/// 30 ratios spread evenly over `center ± spread`.
fn around(center: f64, spread: f64) -> Vec<f64> {
    (0..30)
        .map(|i| center - spread + 2.0 * spread * (i as f64) / 29.0)
        .collect()
}

fn write_result(dir: &Path, routes: Vec<serde_json::Value>) -> PathBuf {
    let file = dir.join("result.json");
    let doc = serde_json::json!({
        "tool": "bench-paired",
        "version": 1,
        "config": { "labels": ["rails", "ruby"] },
        "routes": routes,
    });
    fs::write(&file, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    file
}

#[test]
fn verdicts_follow_the_interval_against_the_tolerance() {
    let dir = scratch("verdicts");
    let mut outlier = around(1.0, 0.004);
    outlier[7] = 5.0;
    let file = write_result(
        &dir,
        vec![
            route("/slow", &around(1.20, 0.01)),
            route("/fast", &around(0.50, 0.01)),
            route("/flat", &around(1.00, 0.005)),
            route("/edge", &around(1.03, 0.02)),
            route("/outlier", &outlier),
        ],
    );
    let out_file = dir.join("summary.json");
    let out = run(&[
        "--analyze",
        file.to_str().unwrap(),
        "--out",
        out_file.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{out:?}");
    let table = stdout(&out);
    assert!(row(&table, "/slow").contains("slower"), "{table}");
    assert!(row(&table, "/fast").contains("faster"), "{table}");
    assert!(row(&table, "/flat").contains("same"), "{table}");
    // The interval straddles 1.03, so neither "same" nor "slower" is supported.
    assert!(row(&table, "/edge").contains("inconclusive"), "{table}");
    assert!(table.contains("rails") && table.contains("ruby time / rails time"), "{table}");

    let summary: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&out_file).unwrap()).unwrap();
    let routes = summary["summary"]["routes"].as_array().unwrap();
    let outlier = routes.iter().find(|r| r["path"] == "/outlier").unwrap();
    // Tukey's fences drop the one 5x pair, so the route reads as unchanged...
    assert_eq!(outlier["outliers"], 1);
    assert_eq!(outlier["verdict"], "same");
    // ...but the batch's CV, taken before the fences, still flags it.
    assert_eq!(outlier["stable"], false);
    assert_eq!(summary["summary"]["unstable"], serde_json::json!(["/outlier"]));
    let fast = routes.iter().find(|r| r["path"] == "/fast").unwrap();
    assert!((fast["ratio"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn fail_on_slower_gates_only_supported_regressions() {
    let dir = scratch("gate");
    let file = write_result(
        &dir,
        vec![route("/a", &around(1.00, 0.005)), route("/b", &around(1.10, 0.01))],
    );
    let path = file.to_str().unwrap();
    assert_eq!(run(&["--analyze", path]).status.code(), Some(0));
    assert_eq!(run(&["--analyze", path, "--fail-on-slower"]).status.code(), Some(1));
    // A 15% tolerance absorbs the 10% slowdown.
    let wide = run(&["--analyze", path, "--fail-on-slower", "--tolerance", "0.15"]);
    assert_eq!(wide.status.code(), Some(0), "{}", stdout(&wide));
    assert!(row(&stdout(&wide), "/b").contains("same"));
    fs::remove_dir_all(dir).unwrap();
}

/// Two loopback servers in one process. The second answers 404 for
/// `/missing`, so a route the two servers disagree on can be tried.
fn servers(dir: &Path) -> (Child, u16, u16) {
    let source = dir.join("servers.rb");
    fs::write(
        &source,
        r##"require 'socket'
def serve(server, missing)
  Thread.new do
    loop do
      client = server.accept
      line = client.gets.to_s
      while (l = client.gets) && l != "\r\n"; end
      status = missing && line.include?('/missing') ? '404 Not Found' : '200 OK'
      client.write "HTTP/1.1 #{status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
      client.close
    end
  end
end
a = TCPServer.new('127.0.0.1', 0)
b = TCPServer.new('127.0.0.1', 0)
puts "#{a.addr[1]} #{b.addr[1]}"
$stdout.flush
[serve(a, false), serve(b, true)].each(&:join)
"##,
    )
    .unwrap();
    let mut child = Command::new("ruby")
        .arg(&source)
        .stdout(Stdio::piped())
        .spawn()
        .expect("ruby is on PATH");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let mut ports = line.split_whitespace().map(|p| p.parse::<u16>().unwrap());
    (child, ports.next().unwrap(), ports.next().unwrap())
}

/// A `wrk` stand-in: 1000 req/s on the reference port, 4000 on any other,
/// logging each URL it was asked to time.
fn stub_wrk(dir: &Path, reference_port: u16) -> PathBuf {
    let wrk = dir.join("wrk");
    fs::write(
        &wrk,
        format!(
            r#"#!/bin/sh
for url; do :; done
echo "$url" >> "{log}"
case "$url" in
  *:{reference_port}/*) rps=1000 ;;
  *) rps=4000 ;;
esac
printf 'Running 1s test @ %s\n  %s requests in 1.00s, 2.00KB read\nRequests/sec: %s\nTransfer/sec: 2.00KB\n' "$url" "$rps" "$rps"
"#,
            log = dir.join("wrk.log").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrk, fs::Permissions::from_mode(0o755)).unwrap();
    wrk
}

#[test]
fn measures_in_alternating_pairs_after_a_matching_preflight() {
    let dir = scratch("measure");
    let (mut child, reference_port, target_port) = servers(&dir);
    let wrk = stub_wrk(&dir, reference_port);
    let reference = format!("http://127.0.0.1:{reference_port}");
    let target = format!("http://127.0.0.1:{target_port}/");
    let out_file = dir.join("out.json");
    let out = run(&[
        "--reference", &reference,
        "--target", &target,
        "--path", "/articles",
        "--pairs", "4",
        "--warmup-pairs", "1",
        "--wrk", wrk.to_str().unwrap(),
        "--out", out_file.to_str().unwrap(),
    ]);
    let mismatch = run(&[
        "--reference", &reference,
        "--target", &target,
        "--path", "/missing",
        "--wrk", wrk.to_str().unwrap(),
    ]);
    child.kill().ok();
    child.wait().ok();

    assert!(out.status.success(), "{out:?}");
    let table = stdout(&out);
    assert!(row(&table, "/articles").contains("0.250"), "{table}");
    assert!(row(&table, "/articles").contains("faster"), "{table}");

    // One warm-up pair, then four measured pairs, order alternating.
    let log = fs::read_to_string(dir.join("wrk.log")).unwrap();
    let sides: Vec<_> = log
        .lines()
        .map(|url| if url.contains(&format!(":{reference_port}/")) { "r" } else { "t" })
        .collect();
    assert_eq!(sides, ["r", "t", "r", "t", "t", "r", "r", "t", "t", "r"]);
    assert!(log.lines().all(|url| url.ends_with("/articles")), "{log}");
    assert!(!log.contains("//articles"), "base URL slash doubled:\n{log}");

    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&out_file).unwrap()).unwrap();
    let pairs = doc["routes"][0]["attempts"][0]["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 4);
    assert_eq!(pairs[1]["order"], "target-first");
    assert_eq!(doc["routes"][0]["statuses"]["target"], 200);
    assert_eq!(doc["summary"]["routes"][0]["stable"], true);

    // A route the servers answer differently is refused, not timed.
    assert_eq!(mismatch.status.code(), Some(2));
    let err = String::from_utf8_lossy(&mismatch.stderr);
    assert!(err.contains("reference answered 200, target answered 404"), "{err}");
    fs::remove_dir_all(dir).unwrap();
}
