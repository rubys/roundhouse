use std::path::Path;
use std::process::Command;

#[test]
fn utc_zone_identity_matches_rails_without_changing_other_zero_offsets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for parser in [
        "runtime/spinel/active_support_time_parsing.rb",
        "runtime/spinel/scaffold/ruby_overlay/runtime/active_support_time_parsing.rb",
    ] {
        let output = Command::new("ruby")
            .arg(root.join("tests/utc_zone_time.rb"))
            .arg(root.join("runtime/ruby/active_support_ext.rb"))
            .arg(root.join(parser))
            .env("TZ", "UTC")
            .output()
            .expect("Ruby on PATH");
        assert!(
            output.status.success(),
            "{parser}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
