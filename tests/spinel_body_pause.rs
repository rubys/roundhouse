//! A request body that pauses part way is still read, as long as the
//! pause is shorter than `Tep::BODY_READ_TIMEOUT` (30s, Puma's
//! first_data_timeout).
//!
//! The spinel lane's three body drains waited 5s for the next bytes. A
//! phone uploading a photo over a mobile link pauses longer than that,
//! and the server closed the connection with half the body read.
//!
//! The driver (`tests/spinel_body_pause.rb`) runs the real servers under
//! CRuby (`tests/tep_server_harness.rb`) with one modeled silence on the
//! wire: a 7s pause must reach the app whole, and a pause past the
//! timeout must be dropped rather than dispatched.

use std::path::Path;
use std::process::Command;

const CHECKS: usize = 6;

#[test]
fn a_body_that_pauses_is_read_until_the_body_read_timeout() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/spinel_body_pause.rb"))
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
