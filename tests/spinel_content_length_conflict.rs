//! Regression for Tep request framing across all three servers.
//! The Ruby driver uses the real parser, drains and server implementations
//! over tests/tep_server_harness.rb's scripted socket.

use std::path::Path;
use std::process::Command;

#[test]
fn invalid_or_conflicting_lengths_are_refused_while_parsing() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/spinel_content_length_conflict.rb"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.lines().any(|line| line == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
    let failed: Vec<&str> = stdout
        .lines()
        .filter(|line| line.starts_with("FAIL"))
        .collect();
    assert!(
        failed.is_empty(),
        "{}\n=== stdout ===\n{stdout}",
        failed.join("\n")
    );
    assert!(
        stdout.lines().any(|line| line == "306/306 checks pass"),
        "fewer checks ran than expected\n=== stdout ===\n{stdout}"
    );
}
