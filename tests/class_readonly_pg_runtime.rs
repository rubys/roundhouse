//! PostgreSQL: class readonly, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE notes (
    id bigserial NOT NULL,
    title character varying,
    shop_id integer,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_class_readonly
"#;

const PROBE: &str = r##"class ReadonlyProbe
  def self.lines
    out = []
    Note.create!(title: "a", shop_id: 1)
    note = Note.readonly.where(shop_id: 1).first
    out << "readonly: #{note.readonly?}"
    begin
      note.update!(title: "b")
      out << "update: saved"
    rescue ActiveRecord::ReadOnlyRecord
      out << "update: ReadOnlyRecord"
    end
    out << "db: #{Note.find(note.id).title}"
    out << "plain: #{Note.where(shop_id: 1).first.readonly?}"
    out
  end
end
"##;

const EXPECTED: [&str; 4] = ["readonly: true", "update: ReadOnlyRecord", "db: a", "plain: false"];

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/note", "app/models/readonly_probe"];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
    ("app/models/readonly_probe.rb", PROBE),
];

#[test]
fn a_class_level_readonly_matches_native_rails_on_postgresql() {
    let app = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "class-readonly",
        &FILES,
        &emitted,
        "rh_spec_class_readonly",
        "ReadonlyProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
