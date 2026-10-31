//! PostgreSQL: unique by index name, on the Ruby target against native Rails.
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
    product_id bigint NOT NULL,
    manual boolean DEFAULT false NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    CONSTRAINT memberships_pkey PRIMARY KEY (id)
);

CREATE UNIQUE INDEX idx_memberships_uniq ON memberships USING btree (source_id, shop_id, product_id);

CREATE UNIQUE INDEX idx_memberships_manual ON memberships USING btree (source_id, product_id) WHERE (manual = true);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_unique_by_name
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const MEMBERSHIP: &str = "class Membership < ApplicationRecord\nend\n";

const PROBE: &str = r##"class UniqueByNameProbe
  def self.row(source, product, manual: false)
    { source_id: source, shop_id: 1, product_id: product, manual: manual, created_at: Time.now, updated_at: Time.now }
  end

  def self.lines
    out = []
    Membership.insert_all([row(1, 10), row(1, 11)], unique_by: "idx_memberships_uniq")
    Membership.insert_all([row(1, 10), row(1, 12)], unique_by: "idx_memberships_uniq")
    out << "insert_all by name: #{Membership.count}"
    Membership.insert_all([row(2, 10)], unique_by: :idx_memberships_uniq)
    out << "insert_all by symbol: #{Membership.count}"
    Membership.insert_all([row(1, 10)], unique_by: %i[source_id shop_id product_id])
    out << "insert_all by columns: #{Membership.count}"
    Membership.upsert_all([row(1, 10, manual: true), row(3, 30)], unique_by: "idx_memberships_uniq")
    out << "upsert_all by name: #{Membership.count} #{Membership.where(manual: true).count}"
    Membership.upsert_all([row(1, 40, manual: true)], unique_by: "idx_memberships_manual")
    Membership.upsert_all([row(1, 40, manual: true)], unique_by: "idx_memberships_manual")
    out << "partial index: #{Membership.count} #{Membership.where(manual: true).count}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    "insert_all by name: 3",
    "insert_all by symbol: 4",
    "insert_all by columns: 4",
    "upsert_all by name: 5 1",
    "partial index: 6 2",
];

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/membership", "app/models/unique_by_name_probe"];

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
    all.push(("app/models/unique_by_name_probe.rb", PROBE));
    all
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
fn unique_by_an_index_name_matches_native_rails_on_postgresql() {
    let files = files();
    let overlay = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("unique-by-name", &files, &emitted, "rh_spec_unique_by_name", "UniqueByNameProbe", &LOADS) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
