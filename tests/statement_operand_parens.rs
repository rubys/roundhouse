//! A statement-shaped operand keeps the parentheses its position needs.
//!
//! Two shapes did not parse:
//!
//! - a splatted modifier conditional, `[*(handles unless ready), :z]`,
//!   was written bare, `[*handles unless ready, :z]`, where the modifier
//!   ends the array element;
//! - a multiple assignment as a condition,
//!   `return unless (previous, current = record.status_change)`, lost
//!   its parentheses, and `unless a, b = pair` does not parse.
//!
//! Native Ruby on the same source is the oracle.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const PROBE: &str = r##"module Operandlab
  class Probe
    def self.splat_unless(ready)
      handles = [:x, :y]
      [:a, *(handles unless ready), :z]
    end

    def self.splat_if(pg)
      [*([:id, :order_id] if pg), :name]
    end

    def self.pair(change)
      return :none unless (before, after = change)

      [before, after]
    end

    def self.report
      [
        splat_unless(false),
        splat_unless(true),
        splat_if(true),
        splat_if(false),
        pair(nil),
        pair([1, 2]),
      ]
    end
  end
end
"##;

const REPORT: &str = "Operandlab::Probe.report.each { |r| p r }\n";

#[test]
fn statement_operands_keep_their_parentheses() {
    let dir =
        std::env::temp_dir().join(format!("roundhouse-operand-parens-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let probe = dir.join("probe.rb");
    std::fs::write(&probe, PROBE).unwrap();
    let native = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!(
            "require {:?}\n{REPORT}",
            probe.display().to_string()
        ))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(
        expected, "[:a, :x, :y, :z]\n[:a, :z]\n[:id, :order_id, :name]\n[:name]\n:none\n[1, 2]\n",
        "native control"
    );

    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/operandlab/probe.rb", PROBE)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source = std::fs::read_to_string(emitted.join("app/models/operandlab/probe.rb"))
        .expect("emitted probe");
    let parse = ruby_prism::parse(source.as_bytes());
    let parse_errors: Vec<String> = parse.errors().map(|e| e.message().to_string()).collect();
    assert!(parse_errors.is_empty(), "{parse_errors:?} in:\n{source}");

    let main = emitted.join("main").display().to_string();
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!(
            "require {main:?}\nMain.configure_default_adapter!\nrequire_relative \"app/models/operandlab/probe\"\n{REPORT}"
        ))
        .current_dir(&emitted)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("spawn ruby");
    assert!(
        output.status.success(),
        "emitted program failed in {}:\n{}\n{source}",
        emitted.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected,
        "emitted vs native:\n{source}"
    );
}
