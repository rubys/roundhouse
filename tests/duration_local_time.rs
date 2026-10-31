#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const MODEL: &str = include_str!("support/duration_local_time.rb");
const SCRIPT: &str = r#"
raise "local seconds" unless LocalDurationClock.added.iso8601(6) == "2026-01-02T03:04:07.123456Z"
raise "aliased minutes" unless LocalDurationClock.subtracted.iso8601(6) == "2026-01-02T03:02:05.123456Z"
raise "retained duration" unless LocalDurationClock.retained_duration.iso8601 == "2026-01-02T02:00:00Z"
raise "outer chained seconds" unless LocalDurationClock.chained_added.iso8601(6) == "2026-01-02T03:04:07.123456Z"
raise "inner chained minutes" unless LocalDurationClock.chained_subtracted.iso8601(6) == "2026-01-02T03:02:05.123456Z"
raise "closure fraction" unless LocalDurationClock.captured.iso8601(6) == "2026-01-02T00:00:00.500000Z"
raise "completed iterator" unless LocalDurationClock.block_capture.iso8601(6) == "2026-01-02T00:00:02.000000Z"
puts "local duration time arithmetic passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("app/models/local_duration_clock.rb", MODEL)
}

#[test]
fn uncertain_time_operands_only_use_helpers_on_ruby_family_targets() {
    let source = "class NativeClock < ApplicationRecord\n  def self.diff(value)\n    Time.utc(2026, 1, 2) - value\n  end\n  def self.add(value)\n    Time.utc(2026, 1, 2) + value\n  end\nend\n";
    let schema = "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :native_clocks do |t|\n  end\nend\n";
    for target in [
        BuildTarget::Ruby,
        BuildTarget::Jruby,
        BuildTarget::Spinel,
        BuildTarget::Crystal,
        BuildTarget::Rust,
        BuildTarget::Go,
        BuildTarget::Typescript,
        BuildTarget::TypescriptWorker,
        BuildTarget::Python,
        BuildTarget::Elixir,
        BuildTarget::Kotlin,
        BuildTarget::Swift,
        BuildTarget::CSharp,
    ] {
        let tree = [
            (
                std::path::PathBuf::from("app/models/native_clock.rb"),
                source.as_bytes().to_vec(),
            ),
            (
                std::path::PathBuf::from("db/schema.rb"),
                schema.as_bytes().to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
        roundhouse::session::analyze_and_lower_for(&mut app, target);
        let ruby = roundhouse::emit::ruby::emit_lowered_models(&app)
            .into_iter()
            .map(|f| f.content)
            .collect::<Vec<_>>()
            .join("\n");
        if matches!(
            target,
            BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel
        ) {
            assert!(
                ruby.contains("Time.utc(2026, 1, 2) - ActiveSupport::Duration.time_operand(value)"),
                "{target:?}: {ruby}"
            );
            assert!(
                ruby.contains("Time.utc(2026, 1, 2) + ActiveSupport::Duration.time_operand(value)"),
                "{target:?}: {ruby}"
            );
        } else {
            assert!(
                ruby.contains("Time.utc(2026, 1, 2) - value"),
                "{target:?}: {ruby}"
            );
            assert!(
                ruby.contains("Time.utc(2026, 1, 2) + value"),
                "{target:?}: {ruby}"
            );
            assert!(!ruby.contains("time_operand"), "{target:?}: {ruby}");
        }
    }
    let (emitted, errors) = emit_and_run::empty_app()
        .write("app/models/native_clock.rb", source)
        .write("db/schema.rb", schema)
        .emit(BuildTarget::Crystal);
    assert!(errors.is_empty(), "{errors:#?}");
    let crystal = std::fs::read_to_string(emitted.join("src/models/native_clock.cr")).unwrap();
    assert!(
        crystal.contains("Time.utc(2026_i64, 1_i64, 2_i64) - value"),
        "{crystal}"
    );
    assert!(!crystal.contains("time_operand"), "{crystal}");
}

#[test]
fn duration_locals_and_aliases_use_runtime_time_operands() {
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let ruby = std::fs::read_to_string(emitted.join("app/models/local_duration_clock.rb")).unwrap();
    assert!(
        ruby.contains("at + ActiveSupport::Duration.time_operand(delay)"),
        "{ruby}"
    );
    assert!(
        ruby.contains("at - ActiveSupport::Duration.time_operand(copy)"),
        "{ruby}"
    );
    assert!(ruby.contains("delay.to_i == 7200"), "{ruby}");
    assert_eq!(
        ruby.matches("at + ActiveSupport::Duration.time_operand(delay)")
            .count(),
        5,
        "{ruby}"
    );
    assert_eq!(
        ruby.matches("at - ActiveSupport::Duration.time_operand(copy)")
            .count(),
        2,
        "{ruby}"
    );
}

#[test]
fn runtime_time_operands_handle_reassignments_and_closures() {
    let controls = r#"class DurationControls
  def self.integer
    at = Time.utc(2026, 1, 2)
    at + 7
  end

  def self.custom
    delay = 2.seconds
    CustomClock.new + delay
  end

  def self.reassigned
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    delay = 7
    at + delay
  end

  def self.conditional(condition)
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    delay = 7 if condition
    at + delay
  end

  def self.fractional
    at = Time.utc(2026, 1, 2)
    delay = 0.5.seconds
    at + delay
  end

  def self.calendar
    at = Time.utc(2026, 1, 2)
    delay = 1.day
    at + delay
  end

  def self.captured
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    cb = -> { delay = 0.5 }
    delay = 2.seconds
    cb.call
    at + delay
  end

  def self.indirect_capture
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    cb = -> { delay = 0.5 }
    delay = 2.seconds
    invoke(cb)
    at - delay
  end

  def self.compound_capture
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    cb = -> { delay -= 1.5 }
    delay = 2.seconds
    cb.call
    at + delay
  end

  def self.multiple_capture
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    cb = -> { delay, other = [0.5, 1] }
    delay = 2.seconds
    cb.call
    at + delay
  end

  def self.invoke(callback)
    callback.call
  end

  def self.block_capture
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    [0.5].each { |value| delay = value }
    delay = 2.seconds
    at + delay
  end

  def self.shadowed
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    [7].map { |delay| at + delay }
  end
end

class CustomClock
  def +(duration)
    duration.to_i
  end
end
"#;
    let (emitted, errors) = emit_and_run::empty_app()
        .write("app/models/duration_controls.rb", controls)
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let ruby = std::fs::read_to_string(emitted.join("app/models/duration_controls.rb")).unwrap();
    assert!(!ruby.contains("delay.to_i"), "{ruby}");
    assert!(
        ruby.contains("at + ActiveSupport::Duration.time_operand(delay)"),
        "{ruby}"
    );
    assert!(
        ruby.contains("at - ActiveSupport::Duration.time_operand(delay)"),
        "{ruby}"
    );
    assert!(ruby.contains("CustomClock.new + delay"), "{ruby}");
    assert!(ruby.contains("at + 7"), "{ruby}");
    assert!(!ruby.contains("time_operand(7)"), "{ruby}");
    emit_and_run::empty_app()
        .write("app/models/duration_controls.rb", controls)
        .run_ruby(
            r#"
at = Time.utc(2026, 1, 2)
raise "captured fraction" unless DurationControls.captured == at + 0.5
raise "indirect fraction" unless DurationControls.indirect_capture == at - 0.5
raise "compound fraction" unless DurationControls.compound_capture == at + 0.5
raise "multiple fraction" unless DurationControls.multiple_capture == at + 0.5
raise "block reassignment" unless DurationControls.block_capture == at + 2
"#,
        )
        .assert_passes();
}

#[test]
fn integral_duration_locals_match_rails_in_emitted_ruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn integral_duration_locals_run_natively_with_emitted_rbs() {
    let run = app().run_spinel_with_rbs(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, "local duration time arithmetic passed\n");
}
