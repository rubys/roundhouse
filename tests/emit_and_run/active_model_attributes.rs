//! `include ActiveModel::Attributes` on a plain class: each `attribute
//! :name, :type` reads nil until written, and its writer casts as Rails
//! 8.1's `ActiveModel::Type` does. Expected values are Rails 8.1.4's,
//! run with `include ActiveModel::Attributes` on CRuby 4.0.5.
use super::emit_and_run;

const EVENT: &str = r#"class SignalEvent
  include ActiveModel::Attributes

  attribute :event_name, :string
  attribute :subject_id, :integer
  attribute :flagged, :boolean
  attribute :timestamp, :datetime
end
"#;

const SCRIPT: &str = r#"
require_relative "app/models/signal_event"
e = SignalEvent.new
raise "fresh: #{[e.event_name, e.subject_id, e.flagged, e.timestamp].inspect}" unless [e.event_name, e.subject_id, e.flagged, e.timestamp] == [nil, nil, nil, nil]
values = "events:property_view:42:2026-10-14".split(":")
e.event_name = values[1].split("_").drop(1).join("_")
e.subject_id = values[2]
e.timestamp = values[3]
e.flagged = "0"
raise "string: #{e.event_name.inspect}" unless e.event_name == "view"
raise "integer: #{e.subject_id.inspect}" unless e.subject_id == 42
raise "boolean: #{e.flagged.inspect}" unless e.flagged == false
raise "date-only: #{e.timestamp.inspect}" unless e.timestamp == Time.utc(2026, 10, 14) && e.timestamp.utc?
raise "strftime: #{e.timestamp.strftime("%Y-%m-%d")}" unless e.timestamp.strftime("%Y-%m-%d") == "2026-10-14"
e.subject_id = ""
raise "blank integer: #{e.subject_id.inspect}" unless e.subject_id.nil?
e.subject_id = "12345678901234567890"
raise "integer limit: #{e.subject_id.inspect}" unless e.subject_id == 1234567890123456
e.subject_id = 7
raise "integer kept: #{e.subject_id.inspect}" unless e.subject_id == 7
e.event_name = nil
raise "nil string: #{e.event_name.inspect}" unless e.event_name.nil?
e.flagged = ""
raise "blank boolean: #{e.flagged.inspect}" unless e.flagged.nil?
e.timestamp = "2026-10-14T12:30:05.123+02:00"
raise "offset: #{e.timestamp.inspect}" unless e.timestamp.utc_offset == 7200 && e.timestamp.strftime("%H:%M:%S.%L") == "12:30:05.123" && e.timestamp.to_i == 1791973805
e.timestamp = ""
raise "blank datetime: #{e.timestamp.inspect}" unless e.timestamp.nil?
puts "ActiveModel::Attributes contract passed"
"#;

fn event() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/lib/signal_event.rb", EVENT)
}

#[test]
fn active_model_attributes_read_nil_and_cast_on_write() {
    let run = event().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveModel::Attributes contract passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn active_model_attributes_read_nil_and_cast_on_write_on_spinel() {
    let run = event().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveModel::Attributes contract passed"), "{}", run.stdout);
}

/// A declaration this lowering does not reproduce (here a `default:`)
/// leaves the class as written, and `check` still reports the include.
#[test]
fn active_model_attributes_with_options_stay_reported() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/lib/signal_event.rb",
            "class SignalEvent\n  include ActiveModel::Attributes\n\n  attribute :event_name, :string, default: \"view\"\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("SignalEvent includes unresolved ActiveModel::Attributes")),
        "{errors:?}"
    );
}
