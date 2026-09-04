//! The SPINEL lane's cable heartbeat and its connection registry.
//!
//! The heartbeat used to be a green thread per connection. Once a
//! deployment's connections arrive over minutes their beat phases spread
//! across the interval, and the scheduler's monitor then turns once per
//! beat — 333 turns a second at a thousand sockets, each turn a walk of
//! everyone parked. One thread beating for all of them is one turn per
//! interval; it is also Action Cable's own shape, which is what the
//! comparison lane runs. matz/spinel#4317 has the measurement.
//!
//! What that buys costs a registry, and a registry can keep beating for
//! connections that are gone. A closed connection's driver REFUSES the
//! write, so such a leak is invisible at every socket and shows up only
//! as work — no client-side probe can see it, which is why this test
//! looks at the registry directly.
//!
//! Sibling shape to `spinel_cable_identity.rs`: a Rust test that shells
//! out to a Ruby driver, so the driver is also runnable by hand:
//!
//! ```sh
//! ruby tests/spinel_cable_heartbeat.rb .
//! ```
//!
//! NO GEMS, no server, no socket — but unlike its two siblings this one
//! FAKES the transport rather than stubbing it empty, because here the
//! transport is the subject.

use std::path::Path;
use std::process::Command;

#[test]
fn the_spinel_heartbeat_beats_once_for_every_open_connection_and_forgets_closed_ones() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = root.join("tests/spinel_cable_heartbeat.rb");
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
        stdout.contains("all probes pass"),
        "spinel cable heartbeat diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
}
