//! PostgreSQL: sti subclass callbacks, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE rooms (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    CONSTRAINT rooms_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sti_subclass_callbacks
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ROOM: &str = r#"class Room < ApplicationRecord
  before_save :base_before
  after_commit :base_commit

  def log
    @log ||= []
  end

  def base_before
    log << "base_before"
  end

  def base_commit
    log << "base_commit"
  end
end
"#;

const OPEN_ROOM: &str = r#"class OpenRoom < Room
  before_save :open_before
  before_save :open_guarded, if: :guarded?
  after_commit :open_created, on: :create
  after_commit :open_destroyed, on: [:destroy]
  after_update_commit :open_updated
  after_commit :open_any

  def guarded?
    name == "g"
  end

  def open_before
    log << "open_before"
  end

  def open_guarded
    log << "open_guarded"
  end

  def open_created
    log << "open_created"
  end

  def open_destroyed
    log << "open_destroyed"
  end

  def open_updated
    log << "open_updated"
  end

  def open_any
    log << "open_any"
  end
end
"#;

const CLOSED_ROOM: &str = r#"class ClosedRoom < Room
end
"#;

const PROBE: &str = r##"class StiSubclassCallbacksProbe
  def self.lines
    out = []
    a = OpenRoom.create!(name: "a")
    out << "create: #{a.log.join(",")}"
    g = OpenRoom.create!(name: "g")
    out << "guarded: #{g.log.join(",")}"
    a.log.clear
    a.update!(name: "b")
    out << "update: #{a.log.join(",")}"
    a.log.clear
    a.destroy
    out << "destroy: #{a.log.join(",")}"
    c = ClosedRoom.create!(name: "c")
    r = Room.create!(name: "r")
    out << "others: #{c.log.join(",")} #{r.log.join(",")} #{Room.count} #{OpenRoom.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    "create: base_before,open_before,open_any,open_created,base_commit",
    "guarded: base_before,open_before,open_guarded,open_any,open_created,base_commit",
    "update: base_before,open_before,open_any,open_updated,base_commit",
    "destroy: open_any,open_destroyed,base_commit",
    "others: base_before,base_commit base_before,base_commit 3 1",
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/room",
    "app/models/open_room",
    "app/models/closed_room",
    "app/models/sti_subclass_callbacks_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/room.rb", ROOM),
    ("app/models/open_room.rb", OPEN_ROOM),
    ("app/models/closed_room.rb", CLOSED_ROOM),
    ("app/models/sti_subclass_callbacks_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn sti_subclass_callbacks_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-subclass-callbacks",
        &FILES,
        &emitted,
        "rh_spec_sti_subclass_callbacks",
        "StiSubclassCallbacksProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
