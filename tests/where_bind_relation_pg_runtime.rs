//! PostgreSQL: where bind relation, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE accounts (
    id bigserial NOT NULL,
    label character varying NOT NULL,
    tier character varying,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);

CREATE TABLE tiers (
    id bigserial NOT NULL,
    name character varying NOT NULL,
    premium boolean NOT NULL DEFAULT false,
    CONSTRAINT tiers_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_where_bind_relation
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const PROBE: &str = r##"class WhereBindRelationProbe
  def self.lines
    out = []
    Tier.create!(name: "gold", premium: true)
    Tier.create!(name: "tin", premium: false)
    Account.create!(label: "a", tier: "gold")
    Account.create!(label: "b", tier: "tin")
    Account.create!(label: "c", tier: nil)
    premium = Tier.where(premium: true).select(:name)
    out << "eq: #{Account.where("tier = (?)", premium.limit(1)).order(:id).pluck(:label).inspect}"
    out << "in: #{Account.where("tier IN (?)", premium).order(:id).pluck(:label).inspect}"
    out << "not in: #{Account.where("tier NOT IN (?)", premium).order(:id).pluck(:label).inspect}"
    out << "named: #{Account.where("tier IN (:names)", names: premium).order(:id).pluck(:label).inspect}"
    out << "and a value: #{Account.where("tier IN (?) AND label = ?", premium, "a").order(:id).pluck(:label).inspect}"
    out
  end
end
"##;

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/account",
    "app/models/tier",
    "app/models/where_bind_relation_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/account.rb", "class Account < ApplicationRecord\nend\n"),
    ("app/models/tier.rb", "class Tier < ApplicationRecord\nend\n"),
    ("app/models/where_bind_relation_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
fn a_relation_bind_is_its_sql_like_native_rails() {
    let (emitted, _errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "where-bind-relation",
        &FILES,
        &emitted,
        "rh_spec_where_bind_relation",
        "WhereBindRelationProbe",
        &LOADS,
    ) else { return };
    // Not a vacuous agreement: Rails' own answers.
    assert_eq!(native[0], r#"eq: ["a"]"#, "{native:#?}");
    assert_eq!(native[1], r#"in: ["a"]"#, "{native:#?}");
    assert_eq!(native[2], r#"not in: ["b"]"#, "{native:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
