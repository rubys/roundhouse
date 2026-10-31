#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n",
        )
        .write(
            "app/models/upload_clock.rb",
            r#"class UploadClock
  MAX_AGE = 1.day
  SHIFT = 2.seconds

  def self.cutoff(now: Time.current)
    now - MAX_AGE
  end

  def self.shifted(now: Time.current)
    now + SHIFT
  end
end
"#,
        )
}

const ASSERTIONS: &str = r#"now = Time.at(100000)
raise "day" unless UploadClock.cutoff(now: now).to_i == 13600
raise "shift" unless UploadClock.shifted(now: now).to_i == 100002
puts "PASS"
"#;

#[test]
fn named_durations_are_converted_for_time_arithmetic() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let source = std::fs::read_to_string(emitted.join("app/models/upload_clock.rb")).unwrap();
    assert!(source.contains("MAX_AGE.to_i"), "{source}");
    assert!(source.contains("SHIFT.to_i"), "{source}");
}

#[test]
fn named_duration_values_match_time_arithmetic_on_ruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

fn test_app() -> emit_and_run::Overlay {
    app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("test/upload_clock_test.rb", r#"require "test_helper"
class UploadClockTest < ActiveSupport::TestCase
  setup do
    @now = Time.at(10) + UploadClock::SHIFT
  end

  test "shifted time" do
    assert_equal 12, (Time.at(10) + UploadClock::SHIFT).to_i
    assert_shifted_time
  end

  def assert_shifted_time
    assert_equal 8, (Time.at(10) - UploadClock::SHIFT).to_i
  end
end
"#)
}

#[test]
fn named_durations_are_converted_in_emitted_test_bodies() {
    let (emitted, errors) = test_app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let source = std::fs::read_to_string(emitted.join("test/upload_clock_test.rb")).unwrap();
    assert!(source.matches("UploadClock::SHIFT.to_i").count() >= 3, "{source}");
}

#[test]
fn named_durations_run_in_emitted_test_bodies() {
    test_app().run_test("test/models/upload_clock_test.rb").assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn named_durations_run_in_native_test_bodies() {
    test_app()
        .write("app/controllers/articles_controller.rb", "class ArticlesController < ActionController::Base\nend\n")
        .write("app/controllers/comments_controller.rb", "class CommentsController < ActionController::Base\nend\n")
        .run_spinel_with_rbs("require_relative 'test/upload_clock_test'\n")
        .assert_passes();
}

#[test]
fn a_numeric_constant_in_a_nearer_namespace_keeps_fractional_seconds() {
    app()
        .write(
            "app/models/scoped_clock.rb",
            r#"class ClockOffset
  VALUE = 2.seconds
end

module ClockNamespace
  class ClockOffset
    VALUE = 0.5
  end

  class ScopedClock
    def self.shifted
      Time.at(10) + ClockOffset::VALUE
    end
  end
end
"#,
        )
        .run_ruby("raise \"fractional seconds changed\" unless ClockNamespace::ScopedClock.shifted.to_f == 10.5\n")
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn named_durations_run_with_emitted_rbs() {
    app().run_spinel_with_rbs(ASSERTIONS).assert_passes();
}
