//! `Base64.urlsafe_encode64` / `urlsafe_decode64` in the ruby-family
//! runtime shim.
//!
//! The shim is deliberately grown on demand (its own header says so),
//! and campfire's QR-code helper is the demand:
//! `Base64.urlsafe_encode64(url)` in the helper, `urlsafe_decode64` in
//! the controller. Without them the constant resolved — we define
//! `Base64` ourselves, which correctly suppresses the bundled-library
//! require — but the method did not exist, and the spinel build stopped
//! there.
//!
//! Behaviour is pinned against CRuby's own stdlib rather than restated:
//! encode is byte-identical across all 256 byte values, and the shim
//! decodes what the stdlib encodes.

use std::process::Command;

#[test]
fn urlsafe_matches_the_cruby_stdlib() {
    let script = r#"
require "base64"
load "runtime/spinel/base64.rb"
inputs = [
  "https://example.com/rooms/1?a=b&c=d",
  "\xff\xfe\x00binary".b,
  "", "a", "ab", "abc",
  "campfire!~?/+",
  (0..255).map(&:chr).join.b,
]
bad = []
inputs.each do |s|
  mine   = Base64.urlsafe_encode64(s)
  theirs = ::Base64.urlsafe_encode64(s)
  bad << "encode #{s.inspect[0, 30]}" if mine != theirs
  bad << "roundtrip #{s.inspect[0, 30]}" if Base64.urlsafe_decode64(mine).b != s.b
  # and it must read what the stdlib wrote
  bad << "decode #{s.inspect[0, 30]}" if Base64.urlsafe_decode64(theirs).b != s.b
end
print bad.empty? ? "OK" : bad.join("; ")
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "OK",
        "stdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}
