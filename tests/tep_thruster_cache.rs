//! Tep::ThrusterCache: Thruster's response cache, in process, for the
//! spinel binary (`runtime/spinel/tep/thruster_cache.rb`). Each rule the
//! module's header claims, held to it through the real threaded server,
//! by the driver `tests/tep_thruster_cache.rb` over the scripted socket
//! in `tests/tep_server_harness.rb`. The CRuby overlay's copy of the same
//! rules is `tests/thruster_cache.rs`.

use std::path::Path;
use std::process::Command;

const CHECKS: usize = 25;

#[test]
fn the_binarys_response_cache_follows_thrusters_rules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/tep_thruster_cache.rb"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // `done` is the last line; its absence means the driver died part
    // way, which an absence of FAIL lines alone would read as a pass.
    assert!(
        stdout.lines().any(|l| l == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    let failed: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "{}\n=== stdout ===\n{stdout}", failed.join("\n"));
    assert!(
        stdout.lines().any(|l| l == format!("{CHECKS}/{CHECKS} checks pass")),
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
}
