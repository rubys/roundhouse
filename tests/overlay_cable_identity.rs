//! The CRuby overlay's `/cable` connection identity, exercised.
//!
//! `Cable.identify` runs the APP's own
//! `ApplicationCable::Connection#connect` against a cookie jar built
//! from the WebSocket handshake, and `config.ru` turns a refusal into a
//! 401 rather than an anonymous socket. Nothing else covers it:
//! campfire's suite reaches channels through Rails'
//! `ActionCable::Channel::TestCase`, which asserts `stream_for` was
//! called and is fully compatible with having no connection identity at
//! all — so the suite cannot see this either way.
//!
//! Overlay-only code, so it cannot live in `runtime/ruby/test/`: those
//! files transpile to every target, and eight of them have no cable.
//! Same shape as `overlay_sanitize_autolink.rs` instead — the driver
//! loads the overlay files directly, in boot.rb's order, so
//! `ruby tests/overlay_cable_identity.rb .` reproduces it by hand.

use std::path::Path;
use std::process::Command;

#[test]
fn the_overlay_identifies_a_cable_connection() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = root.join("tests/overlay_cable_identity.rb");
    let out = Command::new("ruby")
        .arg(&driver)
        .arg(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // A vendor line is always printed; its absence means the driver
    // died before the first assertion, which `status.success()` alone
    // would report as the same failure as a wrong value.
    assert!(
        stdout.contains("vendor "),
        "driver produced no vendor line — it failed before asserting anything\n\
         === stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        stdout.contains("ALL OK"),
        "overlay cable identity diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
}
