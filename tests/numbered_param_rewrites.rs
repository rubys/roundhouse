//! Rewrites that would move a numbered block parameter where Ruby
//! cannot have it leave the call as written.
//!
//! - `rows.map { _1.values_at(*keys) }` was rewritten to
//!   `keys.map { |__k| _1[__k] }`, and a block with an ordinary
//!   parameter cannot read the outer block's `_1`.
//! - `value.tap { _1.delete!("\u200b") }` was rewritten to
//!   `_1 = _1.delete(…)`, and `_1` cannot be assigned; the tapped
//!   string was also left unchanged.
//!
//! Native Ruby on the same source is the oracle.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const PROBE: &str = r##"module Numlab
  class Probe
    def self.values(rows, keys)
      rows.map { _1.values_at(*keys) }
    end

    def self.clean(value)
      value&.strip&.tap { _1.delete!("\u200b") }
    end

    def self.report
      [
        values([{ a: 1, b: 2 }, { a: 3 }], [:b, :a]),
        clean(" a\u200bb "),
        clean(nil),
      ]
    end
  end
end
"##;

const REPORT: &str = "Numlab::Probe.report.each { |r| p r }\n";

#[test]
fn numbered_parameters_stay_where_ruby_reads_them() {
    let dir =
        std::env::temp_dir().join(format!("roundhouse-numbered-param-{}", std::process::id()));
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
        expected, "[[2, 1], [nil, 3]]\n\"ab\"\nnil\n",
        "native control"
    );

    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/numlab/probe.rb", PROBE)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source =
        std::fs::read_to_string(emitted.join("app/models/numlab/probe.rb")).expect("emitted probe");
    let parse = ruby_prism::parse(source.as_bytes());
    let parse_errors: Vec<String> = parse.errors().map(|e| e.message().to_string()).collect();
    assert!(parse_errors.is_empty(), "{parse_errors:?} in:\n{source}");

    let main = emitted.join("main").display().to_string();
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!(
            "require {main:?}\nMain.configure_default_adapter!\nrequire_relative \"app/models/numlab/probe\"\n{REPORT}"
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
