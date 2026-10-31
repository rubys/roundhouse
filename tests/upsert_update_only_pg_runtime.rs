//! PostgreSQL: upsert update only, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE rates (
    id bigserial NOT NULL,
    code character varying NOT NULL,
    value integer,
    batch integer,
    note character varying,
    CONSTRAINT rates_pkey PRIMARY KEY (id)
);
CREATE UNIQUE INDEX index_rates_on_code ON rates USING btree (code);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_upsert_update_only
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";

const RATE: &str = "class Rate < ApplicationRecord\nend\n";

const PROBE: &str = r##"class UpsertUpdateOnlyProbe
  def self.row
    r = Rate.find_by(code: "usd")
    [r.value, r.batch, r.note].inspect
  end

  def self.lines
    out = []
    Rate.upsert({ code: "usd", value: 1, batch: 1, note: "first" }, unique_by: :code)
    out << "insert: #{row}"
    Rate.upsert({ code: "usd", value: 2, batch: 2, note: "second" }, unique_by: :code, update_only: [:value, :batch])
    out << "update_only list: #{row}"
    Rate.upsert({ code: "usd", value: 3, batch: 3, note: "third" }, unique_by: :code, update_only: :value)
    out << "update_only symbol: #{row}"
    Rate.upsert({ code: "usd", value: 4, batch: 4, note: "fourth" }, unique_by: :code)
    out << "default: #{row}"
    Rate.upsert_all([{ code: "usd", value: 5, batch: 5, note: "fifth" }, { code: "eur", value: 6, batch: 6, note: "x" }], unique_by: :code, update_only: [:note])
    out << "upsert_all: #{row} #{Rate.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    r#"insert: [1, 1, "first"]"#,
    r#"update_only list: [2, 2, "first"]"#,
    r#"update_only symbol: [3, 2, "first"]"#,
    r#"default: [4, 4, "fourth"]"#,
    r#"upsert_all: [4, 4, "fifth"] 2"#,
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/rate.rb", RATE),
    ("app/models/upsert_update_only_probe.rb", PROBE),
];

#[test]
fn upsert_update_only_assigns_only_the_listed_columns_as_natively_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "upsert-update-only",
        &FILES,
        &emitted,
        "rh_spec_upsert_update_only",
        "UpsertUpdateOnlyProbe",
        &["app/models/application_record", "app/models/rate", "app/models/upsert_update_only_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
