//! PostgreSQL: time with zone, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE widgets (
    id bigserial NOT NULL,
    name character varying,
    happened_at timestamp(6) without time zone,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_time_with_zone
"#;

const APPLICATION: &str = r#"Bundler.require(*Rails.groups)

module ZoneProbe
  class Application < Rails::Application
    config.time_zone = "Eastern Time (US & Canada)"
  end
end
"#;

/// What railties and Active Record's railtie do natively at boot for
/// that config (the driver loads no Rails application).
const NATIVE_BOOT: &str = r#"require "active_support/all"
Time.zone_default = Time.find_zone!("Eastern Time (US & Canada)")
ActiveRecord::Base.time_zone_aware_attributes = true
ActiveRecord.default_timezone = :utc
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const PROBE: &str = r##"class TimeZoneProbe
  def self.lines
    out = []
    out << "default: #{Time.zone_default.name} #{Time.zone.name} #{Time.zone.now.class} #{Time.zone.now.time_zone.name}"
    Time.zone = "Pacific Time (US & Canada)"
    out << "assigned: #{Time.zone.name} #{Time.zone.formatted_offset}"
    Time.zone = nil
    out << "reset: #{Time.zone.name}"
    t = Time.zone.local(2024, 3, 10, 1, 30)
    out << "local: #{t.inspect} #{t.class} #{t.is_a?(Time)} #{Time === t} #{t.zone} #{t.utc_offset} #{t.dst?}"
    out << "plus: #{(t + 3600).inspect} | #{1.day.since(t).inspect} | #{1.hour.since(t).iso8601} | #{1.month.since(t).inspect}"
    out << "minus: #{1.month.ago(t).inspect} | #{1.day.since(t) - t} | #{(t - 3600).inspect} | #{2.hours.ago(t).inspect} | #{t - Time.utc(2024, 3, 10)}"
    out << "use_zone: #{Time.use_zone("Tokyo") { "#{Time.zone.name} #{Time.zone.parse("2024-01-15 08:00").inspect}" }} #{Time.zone.name}"
    u = Time.utc(2024, 7, 1, 12, 0, 0)
    out << "in_time_zone: #{u.in_time_zone.inspect} | #{u.in_time_zone("Tokyo").inspect} | #{u.in_time_zone.utc.inspect} | #{t.in_time_zone("UTC").inspect}"
    out << "formats: #{t} | #{t.iso8601(3)} | #{t.as_json} | #{t.strftime("%Z %z")} | #{t.to_fs(:long)} | #{t.xmlschema} | #{t.to_json} | #{t.httpdate}"
    out << "compare: #{t == t.utc} #{t.eql?(t.utc)} #{u <=> u.in_time_zone} #{t < t + 1} #{[t + 1, t].min == t} #{t.hash == t.utc.hash} #{t.utc == t}"
    out << "zones: #{Time.zone.at(0).inspect} | #{Time.zone.iso8601("2024-01-15T08:00:00").inspect} | #{Time.zone.parse("2024-06-01 12:00 +0000").inspect} | #{Time.find_zone("Nowhere").inspect}"
    out << "date: #{Date.new(2024, 1, 15).in_time_zone.inspect} | #{"2024-01-15 08:00".in_time_zone.inspect} | #{t.to_date} | #{t.beginning_of_day.inspect} | #{t.end_of_day.inspect}"
    out << "change: #{t.change(hour: 12).inspect} | #{t.advance(months: 1).inspect} | #{t.advance(hours: 2).inspect}"
    d = Date.new(2024, 3, 10)
    out << "date calc: #{d - 30.days} | #{d + 2.weeks} | #{d - 1.month} | #{(d + 1.year).class} | #{d.tomorrow} | #{d.yesterday} | #{Date.tomorrow == Date.current + 1} | #{Date.yesterday == Date.current - 1} | #{d - Date.new(2024, 3, 1)}"
    out << "date weeks: #{d.beginning_of_week} | #{d.end_of_week} | #{d.end_of_month} | #{d.beginning_of_quarter} | #{d.next_weekday} | #{d.prev_weekday} | #{d.next_occurring(:friday)} | #{d.on_weekend?} | #{d.all_week.class} | #{d.quarter}"
    out << "date times: #{d.noon.inspect} | #{d.end_of_day.inspect} | #{d.since(90).inspect} | #{d.ago(60).inspect} | #{(d <=> Time.utc(2024, 3, 10, 5))}"
    # public_send: a Time receiver the analyzer can type is lowered to a
    # zone-aware helper (lower/time_calendar.rs); these call the port's methods.
    out << "time calc: #{u.public_send(:tomorrow).inspect} | #{u.public_send(:yesterday).inspect} | #{(u.public_send(:tomorrow).public_send(:noon) + 1.hour).to_fs(:db)} | #{u.public_send(:beginning_of_week).inspect} | #{u.public_send(:end_of_month).inspect} | #{u.public_send(:next_week).inspect} | #{u.public_send(:days_ago, 3).inspect} | #{u.public_send(:months_since, 1).inspect} | #{u.public_send(:on_weekday?)} | #{u.public_send(:quarter)}"
    widget = Widget.create!(name: "a", happened_at: t)
    reloaded = Widget.find(widget.id)
    out << "column: #{reloaded.happened_at.inspect} #{reloaded.happened_at.class} #{reloaded.happened_at == t} #{Widget.where(happened_at: t).count}"
    Time.use_zone("Tokyo") do
      out << "column tokyo: #{Widget.find(widget.id).happened_at.inspect}"
    end
    cast = Widget.new(name: "b", happened_at: "2024-01-15 08:00")
    out << "string cast: #{cast.happened_at.inspect} #{cast.happened_at.utc.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 20] = [
    "default: Eastern Time (US & Canada) Eastern Time (US & Canada) ActiveSupport::TimeWithZone Eastern Time (US & Canada)",
    "assigned: Pacific Time (US & Canada) -08:00",
    "reset: Eastern Time (US & Canada)",
    "local: 2024-03-10 01:30:00.000000000 EST -05:00 ActiveSupport::TimeWithZone true true EST -18000 false",
    "plus: 2024-03-10 03:30:00.000000000 EDT -04:00 | 2024-03-11 01:30:00.000000000 EDT -04:00 | 2024-03-10T03:30:00-04:00 | 2024-04-10 01:30:00.000000000 EDT -04:00",
    "minus: 2024-02-10 01:30:00.000000000 EST -05:00 | 82800.0 | 2024-03-10 00:30:00.000000000 EST -05:00 | 2024-03-09 23:30:00.000000000 EST -05:00 | 23400.0",
    "use_zone: Tokyo 2024-01-15 08:00:00.000000000 JST +09:00 Eastern Time (US & Canada)",
    "in_time_zone: 2024-07-01 08:00:00.000000000 EDT -04:00 | 2024-07-01 21:00:00.000000000 JST +09:00 | 2024-07-01 12:00:00 UTC | 2024-03-10 06:30:00.000000000 UTC +00:00",
    r#"formats: 2024-03-10 01:30:00 -0500 | 2024-03-10T01:30:00.000-05:00 | 2024-03-10T01:30:00.000-05:00 | EST -0500 | March 10, 2024 01:30 | 2024-03-10T01:30:00-05:00 | "2024-03-10T01:30:00.000-05:00" | Sun, 10 Mar 2024 06:30:00 GMT"#,
    "compare: true true 0 true true true true",
    "zones: 1969-12-31 19:00:00.000000000 EST -05:00 | 2024-01-15 08:00:00.000000000 EST -05:00 | 2024-06-01 08:00:00.000000000 EDT -04:00 | nil",
    "date: 2024-01-15 00:00:00.000000000 EST -05:00 | 2024-01-15 08:00:00.000000000 EST -05:00 | 2024-03-10 | 2024-03-10 00:00:00.000000000 EST -05:00 | 2024-03-10 23:59:59.999999999 EDT -04:00",
    "change: 2024-03-10 12:00:00.000000000 EDT -04:00 | 2024-04-10 01:30:00.000000000 EDT -04:00 | 2024-03-10 04:30:00.000000000 EDT -04:00",
    "date calc: 2024-02-09 | 2024-03-24 | 2024-02-10 | Date | 2024-03-11 | 2024-03-09 | true | true | 9/1",
    "date weeks: 2024-03-04 | 2024-03-10 | 2024-03-31 | 2024-01-01 | 2024-03-11 | 2024-03-08 | 2024-03-15 | true | Range | 1",
    "date times: 2024-03-10 12:00:00.000000000 EDT -04:00 | 2024-03-10 23:59:59.999999999 EDT -04:00 | 2024-03-10 00:01:30.000000000 EST -05:00 | 2024-03-09 23:59:00.000000000 EST -05:00 | -1",
    "time calc: 2024-07-02 12:00:00 UTC | 2024-06-30 12:00:00 UTC | 2024-07-02 13:00:00 | 2024-07-01 00:00:00 UTC | 2024-07-31 23:59:59.999999999 UTC | 2024-07-08 00:00:00 UTC | 2024-06-28 12:00:00 UTC | 2024-08-01 12:00:00 UTC | true | 3",
    "column: 2024-03-10 01:30:00.000000000 EST -05:00 ActiveSupport::TimeWithZone true 1",
    "column tokyo: 2024-03-10 15:30:00.000000000 JST +09:00",
    "string cast: 2024-01-15 08:00:00.000000000 EST -05:00 2024-01-15 13:00:00 UTC",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/application.rb", APPLICATION),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
    ("app/models/time_zone_probe.rb", PROBE),
    ("native_boot.rb", NATIVE_BOOT),
];

fn overlay(files: &[(&str, &str)]) -> emit_and_run::Overlay {
    files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn time_with_zone_answers_as_natively_on_postgresql() {
    let (emitted, errors) = overlay(&FILES).emit(roundhouse::project::BuildTarget::Ruby);
    let facades = std::fs::read_to_string(emitted.join("runtime/gem_facades.rb")).expect("gem_facades.rb");
    for port in ["active_support_time_zone", "active_support_time_with_zone"] {
        assert!(facades.contains(&format!("require_relative \"{port}\"\n")), "{port} not loaded at boot:\n{facades}");
    }
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "time-with-zone",
        &FILES,
        &emitted,
        "rh_spec_time_with_zone",
        "TimeZoneProbe",
        &["native_boot", "app/models/application_record", "app/models/widget", "app/models/time_zone_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
