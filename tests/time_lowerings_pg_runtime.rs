//! PostgreSQL: time lowerings, on the Ruby target against native Rails.
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
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_time_lowerings
"#;

const APPLICATION: &str = r#"Bundler.require(*Rails.groups)

module LoweringProbe
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

const PROBE: &str = r##"class TimeLoweringProbe
  def self.lines
    out = []
    out << "current: #{Time.current.class} #{Time.current.time_zone.name} #{Time.current.utc_offset == Time.zone.now.utc_offset} #{Date.current == Time.zone.today} #{Date.current.class} #{DateTime.current.class} #{Time.zone.today.class} #{Time.zone.tomorrow - Time.zone.yesterday}"
    t = Time.zone.local(2024, 1, 31, 12, 0)
    out << "month: #{(t + 1.month).inspect} | #{(t - 1.month).inspect} | #{(Time.utc(2024, 1, 31, 12) + 1.month).inspect} | #{(t + 1.year).inspect}"
    d = Time.zone.local(2024, 3, 9, 12, 0)
    out << "dst: #{(d + 1.day).inspect} | #{(d + 24.hours).inspect} | #{(d + 1.day) - d} | #{(Time.zone.local(2024, 11, 2, 0, 30) + 1.day).inspect} | #{(d - 1.week).inspect}"
    out << "to_fs: #{t.to_fs(:db)} | #{t.to_formatted_s(:db)} | #{t.to_fs(:number)} | #{t.to_fs(:long)} | #{t.to_fs(:rfc822)} | #{t.to_fs} | #{Time.utc(2024, 1, 31, 17).getlocal("-05:00").to_fs(:db)} | #{Date.new(2024, 3, 8).to_fs(:db)} | #{Date.new(2024, 3, 8).to_fs(:long)}"
    out << "rfc2822: #{t.rfc2822} | #{t.rfc822} | #{Time.utc(2024, 1, 31, 12).rfc2822} | #{t.httpdate} | #{t.to_fs(:rfc2822)}"
    out << "in_time_zone: #{Date.new(2026, 3, 8).in_time_zone("Tokyo").inspect} | #{Date.new(2026, 3, 8).in_time_zone.inspect} | #{Time.utc(2026, 3, 8, 12).in_time_zone("Tokyo").inspect} | #{Time.zone.local(2026, 3, 8, 12).in_time_zone(nil).inspect}"
    out << "string: #{"2026-03-08 12:00".to_time.class} #{"2026-03-08 12:00".to_time.hour} #{"2026-03-08 12:00".to_time(:utc).inspect} | #{"2026-03-08".to_date.inspect} | #{"2026-03-08 12:00".to_datetime.inspect} | #{" ".to_date.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 7] = [
    r#"current: ActiveSupport::TimeWithZone Eastern Time (US & Canada) true true Date DateTime Date 2/1"#,
    r#"month: 2024-02-29 12:00:00.000000000 EST -05:00 | 2023-12-31 12:00:00.000000000 EST -05:00 | 2024-02-29 12:00:00 UTC | 2025-01-31 12:00:00.000000000 EST -05:00"#,
    r#"dst: 2024-03-10 12:00:00.000000000 EDT -04:00 | 2024-03-10 13:00:00.000000000 EDT -04:00 | 82800.0 | 2024-11-03 00:30:00.000000000 EDT -04:00 | 2024-03-02 12:00:00.000000000 EST -05:00"#,
    r#"to_fs: 2024-01-31 17:00:00 | 2024-01-31 17:00:00 | 20240131120000 | January 31, 2024 12:00 | Wed, 31 Jan 2024 12:00:00 -0500 | 2024-01-31 12:00:00 -0500 | 2024-01-31 12:00:00 | 2024-03-08 | March 08, 2024"#,
    r#"rfc2822: Wed, 31 Jan 2024 12:00:00 -0500 | Wed, 31 Jan 2024 12:00:00 -0500 | Wed, 31 Jan 2024 12:00:00 -0000 | Wed, 31 Jan 2024 17:00:00 GMT | Wed, 31 Jan 2024 12:00:00 -0500"#,
    r#"in_time_zone: 2026-03-08 00:00:00.000000000 JST +09:00 | 2026-03-08 00:00:00.000000000 EST -05:00 | 2026-03-08 21:00:00.000000000 JST +09:00 | 2026-03-08 16:00:00 UTC"#,
    r#"string: Time 12 2026-03-08 12:00:00 UTC | Sun, 08 Mar 2026 | Sun, 08 Mar 2026 12:00:00 +0000 | nil"#,
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/application.rb", APPLICATION),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
    ("app/models/time_lowering_probe.rb", PROBE),
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
fn time_lowerings_answer_as_natively_on_postgresql() {
    let (emitted, errors) = overlay(&FILES).emit(roundhouse::project::BuildTarget::Ruby);
    let probe = std::fs::read_to_string(emitted.join("app/models/time_lowering_probe.rb")).expect("probe");
    for lowered in ["Time.now.utc", "getutc.strftime(\"%a, %d %b %Y %H:%M:%S %z\")", "Duration.month(1).to_i", "date_at_midnight"] {
        assert!(!probe.contains(lowered), "{lowered} still lowered:\n{probe}");
    }
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "time-lowerings",
        &FILES,
        &emitted,
        "rh_spec_time_lowerings",
        "TimeLoweringProbe",
        &["native_boot", "app/models/application_record", "app/models/widget", "app/models/time_lowering_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
