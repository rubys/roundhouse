//! `Db.escape_string` writes BYTES as a hex BLOB literal.
//!
//! lobsters' `comments.confidence_order` is a `t.binary` holding
//! `[a, b, c].pack("CCC")`. Quoted as text, a NUL byte ended the SQL
//! ("unrecognized token: \"'\"") on every comment insert, which stopped
//! 73 of its model specs; and a non-NUL binary value stored as TEXT
//! sorts before every BLOB, so the column would order wrong once it held
//! both. Bytes = any NUL, or a BINARY-encoded string that is not plain
//! ASCII. An ASCII-only BINARY string stays text, as it always was.
//!
//! Both ruby-family SQL-literal shims carry the rule (the gem-backed
//! one and the JDBC one). The method is evaluated alone, because loading
//! the whole shim requires the sqlite3 gem, which the unit job does not
//! install.

use std::process::Command;

fn check(shim: &str) {
    let script = format!(
        r##"
src = File.read("{shim}")
defn = src[/^  def self\.escape_string\(s\)\n.*?^  end\n/m] or abort "no escape_string in {shim}"
module Db; end
Db.module_eval(defn)
cases = {{
  [0, 0, 0].pack("CCC")   => "X'000000'",
  [200, 1, 7].pack("CCC") => "X'c80107'",
  "a\0b"                  => "X'610062'",
  "it's"                  => "'it''s'",
  "abc".b                 => "'abc'",
  "héllo"                 => "'héllo'",
  nil                     => "''",
}}
bad = cases.filter_map {{ |v, want| got = Db.escape_string(v); "#{{v.inspect}} -> #{{got}} (want #{{want}})" if got != want }}
print bad.empty? ? "OK" : bad.join("; ")
"##
    );
    let out = Command::new("ruby")
        .arg("-e")
        .arg(&script)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "OK",
        "{shim}: stdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_cruby_shim_writes_bytes_as_a_blob_literal() {
    check("runtime/spinel/db_cruby.rb");
}

#[test]
fn the_jruby_shim_writes_bytes_as_a_blob_literal() {
    check("runtime/spinel/db_jruby.rb");
}
