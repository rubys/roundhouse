//! A binary String reaches SQLite as a BLOB literal. lobsters' comment
//! `confidence_order` is three packed bytes (its default holds NULs), and
//! a quoted literal ended the statement at the first NUL: every comment
//! save on the Spinel tree was "unrecognized token". A binary-tagged
//! String of UTF-8 bytes (a cookie's session token here) stays TEXT.

use super::emit_and_run;
use roundhouse::project::BuildTarget;
use std::process::Command;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_binary_string_escapes_as_a_blob_literal() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors: {}", errors.join("\n"));
    std::fs::write(
        emitted.join("contract.rb"),
        r#"require_relative "runtime/db"
got = [
  Db.escape_string([64729].pack("n") + [7].pack("C")),
  Db.escape_string("\x00\x00\x00".b),
  Db.escape_string("it's"),
  Db.escape_string("café"),
  Db.escape_string((+"\xfc\xd9\x07").force_encoding("UTF-8")),
  Db.escape_string("tok123".b),
]
want = ["X'fcd907'", "X'000000'", "'it''s'", "'café'", "X'fcd907'", "'tok123'"]
raise "escape: #{got.inspect}" unless got == want
puts "binary literal passed"
"#,
    )
    .expect("write contract");
    if std::env::var_os("RH_SPINEL_BUNDLE_COLLECT").is_some() {
        panic!("spinel bundle collect: not bundled");
    }
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let compiled = Command::new(&compiler)
        .args(["contract.rb", "-o", "contract"])
        .current_dir(&*emitted)
        .output()
        .expect("spawn Spinel compiler");
    assert!(
        compiled.status.success(),
        "Spinel compilation failed:\n{}\n{}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(emitted.join("contract")).current_dir(&*emitted).output().expect("run contract");
    assert!(
        output.status.success(),
        "contract failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("binary literal passed"));
}
