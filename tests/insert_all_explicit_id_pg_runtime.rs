//! PostgreSQL: insert all explicit id, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE memberships (
    id bigserial NOT NULL,
    source_id bigint NOT NULL,
    shop_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    CONSTRAINT memberships_pkey PRIMARY KEY (id)
);

CREATE UNIQUE INDEX idx_memberships_uniq ON memberships USING btree (source_id, shop_id);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_insert_all_explicit_id
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const MEMBERSHIP: &str = "class Membership < ApplicationRecord\nend\n";

const PROBE: &str = r##"class InsertAllExplicitIdProbe
  def self.row(id, source)
    { id: id, source_id: source, shop_id: 1, created_at: Time.now, updated_at: Time.now }
  end

  def self.lines
    out = []
    Membership.insert_all([row(900, 1), row(901, 2)])
    out << "insert_all: #{Membership.order(:id).pluck(:id).inspect}"
    Membership.insert_all([row(910, 3)], unique_by: "idx_memberships_uniq")
    out << "insert_all unique_by: #{Membership.order(:id).pluck(:id).inspect}"
    bang = Membership.insert_all!([row(920, 4)], returning: %w[id])
    out << "insert_all! returning: #{bang.rows.inspect}"
    Membership.insert(row(930, 5))
    out << "insert: #{Membership.order(:id).pluck(:id).inspect}"
    Membership.insert_all([{ source_id: 6, shop_id: 1, created_at: Time.now, updated_at: Time.now }])
    out << "allocated: #{Membership.where(source_id: 6).pluck(:id).first > 0}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    "insert_all: [900, 901]",
    "insert_all unique_by: [900, 901, 910]",
    "insert_all! returning: [[920]]",
    "insert: [900, 901, 910, 920, 930]",
    "allocated: true",
];

const LOADS: [&str; 3] =
    ["app/models/application_record", "app/models/membership", "app/models/insert_all_explicit_id_probe"];

const FILES: [(&str, &str); 6] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/membership.rb", MEMBERSHIP),
];

fn files() -> Vec<(&'static str, &'static str)> {
    let mut all = FILES.to_vec();
    all.push(("app/models/insert_all_explicit_id_probe.rb", PROBE));
    all
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
fn insert_all_with_explicit_ids_inserts_them_like_native_rails_on_postgresql() {
    let files = files();
    let overlay = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "insert-all-explicit-id",
        &files,
        &emitted,
        "rh_spec_insert_all_explicit_id",
        "InsertAllExplicitIdProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
