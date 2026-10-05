//! The inbound WebSocket frame-size cap
//! (`runtime/spinel/tep/websocket/frame.rb`).
//!
//! `Tep::WebSocket::DEFAULT_MAX_FRAME` (16 MiB) and
//! `CLOSE_MESSAGE_TOO_BIG` (1009) were both defined, and
//! `Driver#set_max_frame_size` stored a value, but nothing read either:
//! `parse_from_buf` took no cap and the recv loop never consulted one.
//! An oversized frame therefore answered "need more bytes", and
//! `Connection#run` answers "need" by appending the next recv to its
//! accumulator and parsing again — so a client that sent a 14-byte
//! header advertising a 64-bit payload length and then streamed bytes
//! grew that accumulator without bound. That is precisely the OOM the
//! constant's own comment claims it prevents, and campfire holds a green
//! thread per WebSocket, so it was reachable per connection.
//!
//! The driver (`tests/spinel_websocket_frame_cap.rb`) runs under plain
//! CRuby — the codec is pure Ruby (`getbyte`/`pack`/arithmetic) with no
//! `ffi_func` in it, unlike db.rb — so this lane needs no spinel build
//! and is not `#[ignore]`d.

use std::path::Path;
use std::process::Command;

const CHECKS: usize = 15;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn an_oversized_frame_is_refused_from_its_header() {
    let out = Command::new("ruby")
        .arg(root().join("tests/spinel_websocket_frame_cap.rb"))
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
        stdout.lines().any(|l| l == &format!("{CHECKS}/{CHECKS} checks pass")),
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
}

/// The call site, not the codec. Nothing in CI drives `Connection#run`
/// itself, so the one line that hands the parser the driver's cap is
/// unguarded by the Ruby driver above: dropping the argument there
/// restores the unbounded accumulator while every parser check still
/// passes. Asserted structurally because that is what the invariant is —
/// the recv loop's parse MUST be bounded by the connection's cap.
#[test]
fn the_recv_loop_bounds_its_accumulator_by_the_drivers_cap() {
    let src = std::fs::read_to_string(root().join("runtime/spinel/tep/websocket/connection.rb"))
        .expect("read connection.rb");
    let call = src
        .lines()
        .find(|l| l.contains("Frame.parse_from_buf("))
        .expect("the recv loop still parses frames");
    assert!(
        call.contains("max_frame_size"),
        "the recv loop's parse must pass the driver's frame cap, or an \
         oversized frame answers \"need\" and the accumulator grows \
         without bound:\n  {}",
        call.trim()
    );
}
