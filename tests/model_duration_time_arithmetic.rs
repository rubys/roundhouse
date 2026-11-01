#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const MODEL: &str = r#"class RetryClock < ApplicationRecord
  STALE_LEASE = 15.minutes
  SHIFT = 2.seconds

  def self.cutoff
    now = Time.utc(2026, 1, 2, 3, 4, 5)
    now - STALE_LEASE
  end

  def self.shifted
    now = Time.utc(2026, 1, 2, 3, 4, 5)
    now + RetryClock::SHIFT
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :retry_clocks do |t|\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/retry_clock.rb", MODEL)
}

const SCRIPT: &str = r#"
raise "model cutoff" unless RetryClock.cutoff.iso8601 == "2026-01-02T02:49:05Z"
raise "model shift" unless RetryClock.shifted.iso8601 == "2026-01-02T03:04:07Z"
puts "model duration time arithmetic passed"
"#;

#[test]
fn active_record_duration_constants_are_unwrapped_for_time_arithmetic() {
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let ruby = std::fs::read_to_string(emitted.join("app/models/retry_clock.rb")).unwrap();
    assert!(ruby.contains("STALE_LEASE.value"), "{ruby}");
    assert!(ruby.contains("SHIFT.value"), "{ruby}");
}

#[test]
fn duplicate_and_non_duration_model_constants_are_not_stamped_as_durations() {
    let controls = r#"class RetryClock < ApplicationRecord
  DELAY = 2.seconds
  DELAY = 7
  COUNT = 3
  def self.duplicate
    Time.utc(2026, 1, 2) + DELAY
  end
  def self.numeric
    Time.utc(2026, 1, 2) + COUNT
  end
end
"#;
    let (emitted, _) = app()
        .write("app/models/retry_clock.rb", controls)
        .emit(BuildTarget::Spinel);
    let ruby = std::fs::read_to_string(emitted.join("app/models/retry_clock.rb")).unwrap();
    assert!(!ruby.contains("DELAY.to_i"), "{ruby}");
    assert!(!ruby.contains("COUNT.to_i"), "{ruby}");
}

#[test]
fn model_duration_constants_run_in_emitted_ruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn model_duration_constants_run_natively_with_emitted_rbs() {
    let run = app().run_spinel_with_rbs(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, "model duration time arithmetic passed\n");
}
