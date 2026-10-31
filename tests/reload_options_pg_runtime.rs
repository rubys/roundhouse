//! PostgreSQL: reload options, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE things (
    id bigserial NOT NULL,
    name character varying,
    note character varying,
    CONSTRAINT things_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_reload_options
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const RELOAD_FLAG: &str = r#"module ReloadFlag
  attr_reader :reloads

  def reload(*)
    @reloads = (@reloads || 0) + 1
    super
  end
end
"#;

const THING: &str = r#"class Thing < ApplicationRecord
  include ReloadFlag
end
"#;

const PROBE: &str = r#"class ReloadProbe
  def self.lines
    out = []
    thing = Thing.create!(name: "a", note: "n")
    Thing.where(id: thing.id).update_all(name: "b")

    sql = []
    sub = ActiveSupport::Notifications.subscribe("sql.active_record") { |*, payload| sql << payload[:sql] }
    same = Thing.transaction { thing.reload(lock: true) }
    ActiveSupport::Notifications.unsubscribe(sub)
    out << "locked reload: self=#{same.equal?(thing)} name=#{thing.name} reloads=#{thing.reloads} for_update=#{sql.count { |s| s.include?("FOR UPDATE") }}"

    Thing.where(id: thing.id).update_all(name: "c")
    thing.reload
    out << "plain reload: name=#{thing.name} reloads=#{thing.reloads}"

    partial = Thing.select(:id).find(thing.id)
    partial.reload(lock: true)
    out << "partial reload: name=#{partial.name} note=#{partial.note}"

    out
  end
end
"#;

const EXPECTED: [&str; 3] = [
    "locked reload: self=true name=b reloads=1 for_update=1",
    "plain reload: name=c reloads=2",
    "partial reload: name=c note=n",
];

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/reload_flag",
    "app/models/thing",
    "app/models/reload_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/reload_flag.rb", RELOAD_FLAG),
    ("app/models/thing.rb", THING),
    ("app/models/reload_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn reload_options_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{real:?}");
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "reload-options",
        &FILES,
        &emitted,
        "rh_spec_reload_options",
        "ReloadProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
