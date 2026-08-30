//! `ActionCable::Channel::Base` on the SPINEL lane, exercised.
//!
//! Every method on that Base used to raise — "channel subscriptions are
//! not dispatched yet" — which was accurate and is precisely why nothing
//! reported it. campfire ships channel tests and they pass, because
//! Rails' `ActionCable::Channel::TestCase` builds the channel ITSELF and
//! asserts `stream_for` was called: a green `presence_channel_test.rb`
//! is fully compatible with a runtime that cannot build a channel at
//! all, and with a connection that knows nobody.
//!
//! This is issue #71 item 4's foundation on the lane that still lacks
//! it. The CRuby overlay has its own implementation and its own test
//! (`overlay_cable_dispatch`); the two lanes resolve a channel name
//! differently — the overlay by a `REGISTRY` that `self.inherited`
//! fills, which is reflection a static target cannot carry — so they do
//! not share a file and must not share a test.
//!
//! Sibling shape to `overlay_cable_dispatch.rs`: a Rust test that shells
//! out to a Ruby driver, so the driver is also runnable by hand:
//!
//! ```sh
//! ruby tests/spinel_cable_channel.rb .
//! ```
//!
//! NO GEMS, no server, no socket. The driver stubs two empty
//! `Tep::WebSocket` classes so `cable.rb` parses and nothing else — if
//! exercising a channel ever needs the transport, that is a regression
//! in the split rather than a reason to grow the harness.

use std::path::Path;
use std::process::Command;

#[test]
fn a_spinel_channel_streams_rejects_and_names_itself() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = root.join("tests/spinel_cable_channel.rb");
    let out = Command::new("ruby")
        .arg(&driver)
        .arg(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // A vendor line is always printed; its absence means the driver died
    // before the first assertion, which `status.success()` alone would
    // report as the same failure as a wrong value.
    assert!(
        stdout.contains("vendor "),
        "driver produced no vendor line — it failed before asserting anything\n\
         === stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        stdout.contains("ALL OK"),
        "spinel cable channel diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
}
