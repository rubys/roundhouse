#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const MODEL: &str = r#"class FractionalClock
  SHIFT = 1.5.seconds #: ActiveSupport::Duration
  INTEGER = 9007199254740993.seconds

  def self.added
    at = Time.at(10)
    at + 1.5.seconds
  end

  def self.subtracted
    at = Time.at(10)
    at - 1.5.seconds
  end

  def self.named
    Time.at(10) + SHIFT
  end

  def self.local
    delay = 1.5.seconds
    Time.at(10) - delay
  end

  def self.integer
    Time.at(10) + 2.seconds
  end

  def self.large_integer
    Time.at(0) + INTEGER
  end

  def self.minutes
    Time.at(10) + 0.125.minutes
  end
end
"#;

const SCRIPT: &str = r#"
raise "addition" unless FractionalClock.added.to_f == 11.5
raise "subtraction" unless FractionalClock.subtracted.to_f == 8.5
raise "named" unless FractionalClock.named.to_f == 11.5
raise "local" unless FractionalClock.local.to_f == 8.5
raise "integer" unless FractionalClock.integer.to_i == 12
raise "integer precision" unless FractionalClock.large_integer.to_i == 9007199254740993
raise "unit scaling" unless FractionalClock.minutes.to_f == 17.5
puts "fractional and integer duration arithmetic passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("app/models/fractional_clock.rb", MODEL)
}

#[test]
fn time_arithmetic_reads_numeric_duration_seconds() {
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let source = std::fs::read_to_string(emitted.join("app/models/fractional_clock.rb")).unwrap();
    for expression in [
        "at + ActiveSupport::Duration.seconds(1.5).value",
        "at - ActiveSupport::Duration.seconds(1.5).value",
        "Time.at(10) + FractionalClock::SHIFT.value",
        "Time.at(10) + ActiveSupport::Duration.seconds(2).value",
        "Time.at(0) + FractionalClock::INTEGER.value",
    ] {
        assert!(source.contains(expression), "{expression}: {source}");
    }
}

#[test]
fn fractional_and_integer_durations_run_in_emitted_ruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
fn fractional_durations_survive_repeated_test_body_lowering() {
    let test_app = || {
        app()
            .write("roundhouse.yml", "test_paths:\n  - test\n")
            .write(
                "test/fractional_clock_test.rb",
                r#"require "test_helper"
class FractionalClockTest < ActiveSupport::TestCase
  test "fractional duration operands" do
    assert_equal 11.5, (Time.at(10) + FractionalClock::SHIFT).to_f
    assert_equal 8.5, (Time.at(10) - 1.5.seconds).to_f
  end
end
"#,
            )
    };
    let (emitted, errors) = test_app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let source = std::fs::read_to_string(emitted.join("test/fractional_clock_test.rb")).unwrap();
    assert!(
        source.contains("FractionalClock::SHIFT.value"),
        "{source}"
    );
    assert!(
        source.contains("ActiveSupport::Duration.seconds(1.5).value"),
        "{source}"
    );
    assert!(
        !source.contains("Duration.seconds(FractionalClock::SHIFT)"),
        "{source}"
    );
    test_app()
        .run_test("test/models/fractional_clock_test.rb")
        .assert_passes();
}

#[test]
fn strict_targets_still_refuse_duration_values() {
    for target in [BuildTarget::Rust, BuildTarget::Typescript] {
        let (_, errors) = app().emit(target);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("runtime ships none")),
            "{target:?}: {errors:#?}"
        );
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn fractional_and_integer_durations_run_with_emitted_rbs() {
    app().run_spinel_with_rbs(SCRIPT).assert_passes();
}
