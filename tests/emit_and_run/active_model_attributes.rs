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
raise "integer setter: #{(e.subject_id = "08").inspect}" unless (e.subject_id = "08") == "08" && e.subject_id == 8
raise "boolean setter: #{(e.flagged = "0").inspect}" unless (e.flagged = "0") == "0" && e.flagged == false
e.subject_id = ""
raise "blank integer: #{e.subject_id.inspect}" unless e.subject_id.nil?
e.subject_id = "\u00A0"
raise "nbsp integer: #{e.subject_id.inspect}" unless e.subject_id.nil?
e.subject_id = "1_0"
raise "underscore: #{e.subject_id.inspect}" unless e.subject_id == 10
e.subject_id = "1__0"
raise "double underscore: #{e.subject_id.inspect}" unless e.subject_id == 1
e.subject_id = Float::INFINITY
raise "infinity: #{e.subject_id.inspect}" unless e.subject_id.nil?
e.subject_id = Float::NAN
raise "nan: #{e.subject_id.inspect}" unless e.subject_id.nil?
e.subject_id = 3.9
raise "truncate: #{e.subject_id.inspect}" unless e.subject_id == 3
e.subject_id = "12345678901234567890"
raise "integer limit: #{e.subject_id.inspect}" unless e.subject_id == 1234567890123456
e.subject_id = "111111111111111é999"
raise "byte limit: #{e.subject_id.inspect}" unless e.subject_id == 111111111111111
e.subject_id = (" " * 10) + "99"
raise "leading space: #{e.subject_id.inspect}" unless e.subject_id == 99
e.subject_id = ("é" * 40) + "12"
raise "long multibyte: #{e.subject_id.inspect}" unless e.subject_id == 0
e.subject_id = 7
raise "integer kept: #{e.subject_id.inspect}" unless e.subject_id == 7
e.event_name = true
raise "true string: #{e.event_name.inspect} frozen=#{e.event_name.frozen?}" unless e.event_name == "t" && e.event_name.frozen?
raise "string setter: #{(e.event_name = 12).inspect}" unless (e.event_name = 12) == 12 && e.event_name == "12"
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

/// A declaration this lowering does not reproduce leaves the class as
/// written, and `check` still reports the include. An option other than
/// `default:` (or a default it can't read: a lambda that takes a
/// parameter, a computed value), a block, a receiver, a trailing `#:`, and
/// any other `include` are each enough, including when the class also has
/// one plain `attribute :name, :string`.
#[test]
fn active_model_attributes_with_options_stay_reported() {
    let bodies = [
        "attribute :event_name, :string, limit: 3\n",
        "attribute :event_name, :string, default: ->(record) { record }\n",
        "attribute :event_name, :string, default: [\"a\"].first\n",
        "attribute :event_name, :string do\n  end\n",
        "self.attribute :event_name, :string\n",
        "attribute :event_name, :string #: String\n",
        "attribute :event_name, :string\n  attribute :subject_id, :integer do\n  end\n",
        "attribute :event_name, :string\n  self.attribute :subject_id, :integer\n",
        "attribute :event_name, :string\n  attribute :subject_id, :integer #: Integer\n",
        "include Enumerable\n  attribute :event_name, :string\n",
    ];
    for body in bodies {
        let source = format!("class SignalEvent\n  include ActiveModel::Attributes\n\n  {body}end\n");
        let (_emitted, errors) = emit_and_run::real_blog()
            .write("app/lib/signal_event.rb", &source)
            .emit(roundhouse::project::BuildTarget::Ruby);
        assert!(
            errors.iter().any(|e| e.contains("SignalEvent includes unresolved ActiveModel::Attributes")),
            "{body}\n{errors:?}"
        );
    }
}

/// The ruby-family class calls `ActiveSupport.cast_integer` and does
/// not call `Float#finite?`. TypeScript emits the same call and reports
/// it: that runtime does not load `active_support_ext.rb`. Rust and Go
/// do not emit a plain class, and they still say so.
#[test]
fn active_model_attributes_are_clean_without_finite() {
    fn generated_class(dir: &std::path::Path) -> String {
        let mut stack = vec![dir.to_path_buf()];
        let mut out = String::new();
        while let Some(path) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.to_string_lossy().to_ascii_lowercase().contains("signal_event") {
                    out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
                }
            }
        }
        out
    }
    for target in [
        roundhouse::project::BuildTarget::Ruby,
        roundhouse::project::BuildTarget::Typescript,
    ] {
        let (emitted, errors) = event().emit(target);
        assert!(
            errors.iter().all(|e| !e.contains("unresolved ActiveModel::Attributes")),
            "{target:?}: {errors:?}"
        );
        let class = generated_class(emitted.as_ref());
        assert!(class.contains("cast_integer"), "{target:?} writer does not call cast_integer:\n{class}");
        assert!(!class.contains("finite?"), "{target:?} writer calls finite?:\n{class}");
        assert!(!class.contains("super("), "{target:?} constructor calls super:\n{class}");
        if target == roundhouse::project::BuildTarget::Typescript {
            assert!(
                errors.iter().any(|e| e.contains("ActiveSupport.cast_integer is only available")),
                "{target:?} dropped the cast without saying so: {errors:?}"
            );
        }
    }
    for target in [roundhouse::project::BuildTarget::Rust, roundhouse::project::BuildTarget::Go] {
        let (_emitted, errors) = event().emit(target);
        assert!(
            errors.iter().all(|e| !e.contains("unresolved ActiveModel::Attributes")),
            "{target:?}: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("plain Ruby class") && e.contains("SignalEvent")),
            "{target:?} dropped the class without saying so: {errors:?}"
        );
    }
}

const DEFAULTS: &str = r#"class SettingsEvent
  include ActiveModel::Attributes

  CALLS = []

  attribute :notify, :boolean, default: true
  attribute :limit, :integer, default: 10
  attribute :label, :string, default: "none"
  attribute :cleared, :integer, default: nil
  attribute :parsed, :integer, default: "12"
  attribute :off, :boolean, default: "f"
  attribute :stamp, :integer, default: -> { CALLS << :call; CALLS.size }
  attribute :origin, :string, default: -> { name }
end
"#;

/// `attribute :x, :type, default: …` (Rails 8.1.4): the default is cast as
/// an assigned value is; a lambda is called on the attribute's first read,
/// with the class as `self`, and kept; a write before that read skips it;
/// a String default is a fresh String per instance; `default: nil` is no
/// default.
const DEFAULTS_SCRIPT: &str = r##"
require_relative "app/models/settings_event"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
s = SettingsEvent.new
check("boolean", s.notify, true)
check("integer", s.limit, 10)
check("string", s.label, "none")
check("nil", s.cleared, nil)
check("cast integer", s.parsed, 12)
check("cast boolean", s.off, false)
s.notify = "0"
check("write casts", s.notify, false)
s.limit = nil
check("nil overrides", s.limit, nil)
check("fresh string", SettingsEvent.new.label.equal?(SettingsEvent.new.label), false)
SettingsEvent::CALLS.clear
a = SettingsEvent.new
b = SettingsEvent.new
check("lambda not called by new", SettingsEvent::CALLS.size, 0)
check("lambda on first read", [b.stamp, a.stamp], [1, 2])
check("lambda kept", [a.stamp, a.stamp], [2, 2])
c = SettingsEvent.new
c.stamp = 5
check("write before read", [c.stamp, SettingsEvent::CALLS.size], [5, 2])
check("lambda self is the class", SettingsEvent.new.origin, "SettingsEvent")
puts "ActiveModel::Attributes defaults passed"
"##;

fn settings() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/settings_event.rb", DEFAULTS)
}

#[test]
fn active_model_attributes_defaults() {
    let run = settings().run_ruby(DEFAULTS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveModel::Attributes defaults passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn active_model_attributes_defaults_on_spinel() {
    let run = settings().run_spinel(DEFAULTS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveModel::Attributes defaults passed"), "{}", run.stdout);
}
