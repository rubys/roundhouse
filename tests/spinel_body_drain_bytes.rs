//! The request-body drains in the spinel lane's HTTP server count bytes.
//!
//! Content-Length counts bytes. 5cb6d051 moved request.rb's two drains to
//! `bytesize`, but the blocking server's drain, `Sock.sphttp_drain_body`
//! in net.rb, still compared `out.length`. It measured correctly only
//! because `out = out + chunk` keeps recv'd bytes binary on spinel; the
//! same bytes appended with `<<` count characters (probed on spinel
//! 775ba5f68). Had `out` counted characters, a multibyte body would have
//! looked short with every byte in hand and the drain would have read the
//! next pipelined request into this request's body.
//!
//! Only the blocking `Tep::Server` calls this drain, and the scaffold
//! boots the threaded (default) or scheduled server. All three servers
//! are checked here, the other two as regressions.
//!
//! The driver (`tests/spinel_body_drain_bytes.rb`) runs the real servers
//! under CRuby (`tests/tep_server_harness.rb`) in a stress mode that hands
//! recv'd chunks back tagged UTF-8, so `length` counts characters on
//! anything built from them. Under CRuby's binary chunks the confusion is
//! invisible.

use std::path::Path;
use std::process::Command;

const CHECKS: usize = 12;

#[test]
fn a_multibyte_body_is_drained_to_its_byte_count_and_no_further() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/spinel_body_drain_bytes.rb"))
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
