//! Named-channel subscribe dispatch on the CRuby overlay, exercised.
//!
//! `Cable::Dispatch` resolves the channel a subscribe frame NAMED, runs
//! the app's own `subscribed`, and registers only what that method
//! asked for. Before it, a subscribe read `signed_stream_name` out of
//! the identifier and joined whatever it decoded to — which is exactly
//! the bypass campfire prepends `RoomStreamsAreAuthorized` onto
//! `Turbo::StreamsChannel` to close, so the one app in the corpus that
//! had thought about cable authorization had its answer discarded.
//!
//! Nothing else covers it. campfire's suite reaches channels through
//! Rails' `ActionCable::Channel::TestCase`, which instantiates the
//! channel itself and asserts `stream_for` was called — a green
//! `presence_channel_test.rb` is fully compatible with no dispatch at
//! all, and with no connection identity either.
//!
//! Overlay-only code, so it cannot live in `runtime/ruby/test/`: those
//! files transpile to every target and eight of them have no cable.
//! Sibling of `overlay_cable_identity.rs` — same driver shape, and the
//! same gem constraint:
//!
//! THE DRIVER MUST NOT NEED nio4r OR websocket-driver. The unit job
//! installs neither. To run it the way CI does, hide the gems rather
//! than trusting their absence:
//!
//! ```sh
//! mkdir -p /tmp/nogems/websocket
//! echo 'raise LoadError, "cannot load such file -- nio"' > /tmp/nogems/nio.rb
//! echo 'raise LoadError, "x"' > /tmp/nogems/websocket/driver.rb
//! RUBYOPT=-I/tmp/nogems ruby tests/overlay_cable_dispatch.rb .
//! ```

use std::path::Path;
use std::process::Command;

#[test]
fn the_overlay_dispatches_a_subscribe_to_the_channel_it_names() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = root.join("tests/overlay_cable_dispatch.rb");
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
        "overlay cable dispatch diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
}
