//! PostgreSQL: sql binds and integer booleans, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE items (
    id bigserial NOT NULL,
    name character varying NOT NULL,
    is_not_deleted smallint DEFAULT 1,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sql_binds_integer_booleans
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ITEM: &str = "class Item < ApplicationRecord\nend\n";

const PROBE: &str = r##"class SqlBindsIntegerBooleansProbe
  def self.lines
    out = []
    now = Time.now
    Item.create!(name: "live", is_not_deleted: 1, created_at: now, updated_at: now)
    Item.create!(name: "gone", is_not_deleted: 0, created_at: now, updated_at: now)
    out << "true: #{Item.where(is_not_deleted: true).pluck(:name).inspect}"
    out << "false: #{Item.where(is_not_deleted: false).pluck(:name).inspect}"
    out << "not true: #{Item.where.not(is_not_deleted: true).pluck(:name).inspect}"
    out << "list: #{Item.where(is_not_deleted: [true, false]).order(:id).pluck(:name).inspect}"
    id = Item.where(name: "live").pluck(:id).first
    rows = Item.connection.select_all(Arel.sql("SELECT id, ? AS handle FROM items WHERE id = ? AND name = ?", "O'Reilly? --", id, "live")).to_a
    out << "bound: #{rows.map { |r| [r["id"] == id, r["handle"]] }.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    r#"true: ["live"]"#,
    r#"false: ["gone"]"#,
    r#"not true: ["gone"]"#,
    r#"list: ["live", "gone"]"#,
    r#"bound: [[true, "O'Reilly? --"]]"#,
];

const LOADS: [&str; 3] =
    ["app/models/application_record", "app/models/item", "app/models/sql_binds_integer_booleans_probe"];

const FILES: [(&str, &str); 6] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/item.rb", ITEM),
];

fn files() -> Vec<(&'static str, &'static str)> {
    let mut all = FILES.to_vec();
    all.push(("app/models/sql_binds_integer_booleans_probe.rb", PROBE));
    all
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
fn integer_column_booleans_and_bound_arel_sql_match_native_rails_on_postgresql() {
    let files = files();
    let overlay = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sql-binds-integer-booleans",
        &files,
        &emitted,
        "rh_spec_sql_binds_integer_booleans",
        "SqlBindsIntegerBooleansProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
