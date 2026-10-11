//! String literals whose bytes are not UTF-8, against native Ruby.
//!
//! Reduction: a generated file assigns
//! `DATA = "\n\x12\x64\x65vice_token.proto\"\xfb\x01…"`, a serialized
//! binary blob. Ingest read every literal as UTF-8 text, lossily, so each
//! invalid byte was emitted as U+FFFD and the blob no longer parsed. The
//! emitted literal must be the same bytes, in the same encoding, frozen
//! where the source froze it: as a constant and as a method's return
//! value alike.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const DESCRIPTORS: &str = r#"# frozen_string_literal: true

module Descriptors
  DATA = "\n\x12\x64\x65vice_token.proto\"\xfb\x01\n\x0b\x44\x65viceToken\xea\x02$Proto"
  TEXT = "caf\u00e9"

  def self.inline
    "\xff\x00ab"
  end
end
"#;

/// A US-ASCII source: Ruby gives a `\x` escape outside ASCII there the
/// binary encoding (Prism's forced binary), and nothing is frozen.
const LEGACY: &str = "# encoding: us-ascii\nmodule Legacy\n  MARK = \"\\xff\\xfe\"\nend\n";

/// The probe's own lines, for native Ruby and the emitted tree alike.
const SCRIPT: &str = r#"show = ->(s) { [s.unpack1("H*"), s.encoding.to_s, s.valid_encoding?, s.frozen?] }
puts show.(Descriptors::DATA).inspect
puts show.(Descriptors.inline).inspect
puts show.(Legacy::MARK).inspect
puts [Descriptors::TEXT, Descriptors::TEXT.encoding.to_s].inspect
"#;

/// What Ruby prints (checked natively), so two equal wrong answers fail.
const EXPECTED: &str = "\
[\"0a126465766963655f746f6b656e2e70726f746f22fb010a0b446576696365546f6b656eea022450726f746f\", \"UTF-8\", false, true]\n\
[\"ff006162\", \"UTF-8\", false, true]\n\
[\"fffe\", \"ASCII-8BIT\", true, false]\n\
[\"café\", \"UTF-8\"]\n";

#[test]
fn a_string_literal_that_is_not_utf8_keeps_its_bytes_as_in_ruby() {
    let dir = std::env::temp_dir().join(format!("rh-binary-literal-native-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("descriptors.rb"), DESCRIPTORS).unwrap();
    std::fs::write(dir.join("legacy.rb"), LEGACY).unwrap();
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!(
            "require {:?}\nrequire {:?}\n{SCRIPT}",
            dir.join("descriptors.rb").display().to_string(),
            dir.join("legacy.rb").display().to_string()
        ))
        .output()
        .expect("ruby");
    let _ = std::fs::remove_dir_all(&dir);
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    assert!(native.status.success(), "native: {native_out}\n{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(native_out, EXPECTED, "native");

    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("app/models/descriptors.rb", DESCRIPTORS)
        .write("app/models/legacy.rb", LEGACY)
        .run_ruby(SCRIPT);
    assert!(run.errors.is_empty(), "{:#?}", run.errors);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, native_out, "emitted\n{}", run.stderr);
}
