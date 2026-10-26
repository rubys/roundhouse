use std::path::Path;
use std::process::Command;

#[test]
fn unix_listener_lifecycle_and_connection_transport() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for script in [
        "tep_unix_listener.rb",
        "tep_listener_transport.rb",
        "puma_unix_bind.rb",
    ] {
        let out = Command::new("ruby")
            .arg(root.join("tests").join(script))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{script}:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
