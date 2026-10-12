//! A constant written inside another constant's value.
//!
//! An enum-like class declares its members inside the list of them:
//! `ALL = [LOW = T.let(Level.new(…), Level), …].freeze`.
//! Ruby defines LOW in Level when ALL's value runs. Native Ruby
//! on the same source is the oracle for the values, their order and
//! their identity with ALL's elements.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LEVEL: &str = r#"# frozen_string_literal: true

class Level
  attr_reader :value

  def initialize(value:)
    @value = value
  end

  ALL = [
    LOW = Level.new(value: "LOW"),
    Level.new(value: "MID"),
    HIGH = Level.new(value: "HIGH"),
  ].freeze
  BY_VALUE = { (KEY = "low") => LOW }.freeze

  def self.report
    [LOW.value, HIGH.value, ALL.map(&:value), ALL.first.equal?(LOW), BY_VALUE[KEY].equal?(LOW)]
  end
end
"#;

const READER: &str = r#"# frozen_string_literal: true

module Reader
  def self.low
    Level::LOW.value
  end
end
"#;

const REPORT: &str = "p Level.report, Reader.low\n";

#[test]
fn a_constant_written_in_another_constant_s_value_is_declared_as_in_native_ruby() {
    let dir = std::env::temp_dir().join(format!("roundhouse-nested-const-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("level.rb"), LEVEL).unwrap();
    std::fs::write(dir.join("reader.rb"), READER).unwrap();
    let native = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("require {:?}\nrequire {:?}\n{REPORT}", dir.join("level.rb"), dir.join("reader.rb")))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert!(expected.contains("[\"LOW\", \"MID\", \"HIGH\"], true, true]"), "native control: {expected}");

    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/lib/level.rb", LEVEL)
        .write("app/lib/reader.rb", READER)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require \"./app/models/level\"\nrequire \"./app/models/reader\"\n{REPORT}"))
        .current_dir(&emitted)
        .output()
        .expect("spawn ruby");
    assert!(output.status.success(), "emitted program failed in {}:\n{}", emitted.display(), String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

/// A write the value may not evaluate (a branch, a block, the
/// arguments of a `&.` call) does not
/// declare the constant: its reads stay refused.
#[test]
fn a_constant_written_in_a_branch_of_another_s_value_stays_unknown() {
    let source = r#"# frozen_string_literal: true

class Flags
  ON = ENV["FLAG"] ? (LATE = 1) : 2
  LIST = [1].map { |x| x }
  TAILED = ENV["FLAG"]&.concat(TAIL = "x")

  def self.late
    LATE
  end

  def self.tail
    TAIL
  end
end
"#;
    let (_, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/lib/flags.rb", source)
        .emit(BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("constant not supported (all targets): LATE") || e.contains("Flags::LATE")),
        "{}",
        errors.join("\n")
    );
    assert!(errors.iter().any(|e| e.contains("TAIL")), "{}", errors.join("\n"));
}
