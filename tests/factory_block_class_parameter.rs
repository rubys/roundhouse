//! Ruby passes the new class to a `Data.define` block
//! (`do |klass| … end`). A parameter the block never reads changes
//! nothing, so the block is the plain class body. A parameter the block
//! reads, or any other parameter list, stays a named refusal.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

const UNREAD: &str = r#"module Geometry
  Pair = Data.define(:left, :right) do |_klass|
    def empty? = left.nil? && right.nil?
  end
end
"#;

const REPORT: &str = r#"p Geometry::Pair.new(left: nil, right: nil).empty?
p Geometry::Pair.new(left: 1, right: nil).empty?
"#;

#[test]
fn an_unread_class_parameter_on_a_factory_block_is_the_plain_body() {
    let native = Command::new("ruby")
        .arg("-e")
        .arg(format!("{UNREAD}\n{REPORT}"))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(expected, "true\nfalse\n");

    let run = emit_and_run::real_blog().write("app/lib/geometry.rb", UNREAD).run_ruby(REPORT);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
}

#[test]
fn a_read_class_parameter_stays_a_named_refusal() {
    let root = std::env::temp_dir().join(format!("roundhouse-factory-param-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/lib/reads.rb", "module Reads\n  X = Data.define(:a) do |klass|\n    def twice = a * 2\n    klass\n  end\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg("--continue").arg(&root).output().unwrap();
    let _ = std::fs::remove_dir_all(&root);
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("Data.define blocks support only instance methods"), "{text}");
}
