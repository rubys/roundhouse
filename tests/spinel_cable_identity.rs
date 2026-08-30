//! The SPINEL lane's `/cable` connection identity, exercised.
//!
//! `Cable.upgrade` used to welcome every handshake. The app's own
//! `ApplicationCable::Connection#connect` never ran, so campfire's
//! `identified_by :current_user` resolved to nobody and a channel had no
//! user to authorize against — issue #71 item 3, on the lane that still
//! lacked it. The CRuby overlay has its own implementation and its own
//! test (`overlay_cable_identity`); the two lanes cannot share either,
//! because the overlay reaches the app's class through
//! `defined?(ApplicationCable::Connection)` and a target that resolves
//! every call statically has no such lane. The BEHAVIOUR is the shared
//! contract, and the probes in the two drivers are deliberately
//! probe-for-probe.
//!
//! Sibling shape to `spinel_cable_channel.rs`: a Rust test that shells
//! out to a Ruby driver, so the driver is also runnable by hand:
//!
//! ```sh
//! ruby tests/spinel_cable_identity.rb .
//! ```
//!
//! NO GEMS, no server, no socket. The generator half — that
//! `project::apply_cable_connection` writes the arm the driver
//! exercises — is pinned by a unit test in `src/project.rs`, which is
//! where the function lives.

use std::path::Path;
use std::process::Command;

#[test]
fn a_spinel_cable_handshake_is_identified_or_refused() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = root.join("tests/spinel_cable_identity.rb");
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
        "spinel cable identity diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
}
