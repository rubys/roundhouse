//! PostgreSQL: joins alias default scope, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE ledger_accounts (
    id bigserial NOT NULL,
    island bigint NOT NULL,
    migrated_on character varying,
    CONSTRAINT ledger_accounts_pkey PRIMARY KEY (id)
);

CREATE TABLE providers (
    id bigserial NOT NULL,
    island bigint NOT NULL,
    ledger_account_id bigint NOT NULL,
    remote_id bigint NOT NULL,
    CONSTRAINT providers_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_joins_alias_default_scope
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const LEDGER: &str = "class LedgerAccount < ApplicationRecord\n  default_scope { where(island: 0) }\nend\n";
const PROVIDER: &str = r#"class Provider < ApplicationRecord
  belongs_to :ledger_account, class_name: "LedgerAccount"

  def self.migrated(remote_id)
    Provider.where(remote_id: remote_id).joins(:ledger_account).where.not(ledger_account: { migrated_on: nil })
  end

  def self.migrated_braced(remote_id)
    where(remote_id: remote_id).joins(:ledger_account).where.not({ ledger_account: { migrated_on: nil } })
  end
end
"#;

const PROBE: &str = r##"class JoinsAliasDefaultScopeProbe
  def self.lines
    out = []
    moved = LedgerAccount.create!(island: 0, migrated_on: "2026")
    fresh = LedgerAccount.create!(island: 0, migrated_on: nil)
    Provider.create!(island: 0, ledger_account_id: moved.id, remote_id: 7)
    Provider.create!(island: 0, ledger_account_id: fresh.id, remote_id: 7)
    out << "where.not: #{Provider.migrated(7).map(&:ledger_account_id).sort == [moved.id]} #{Provider.migrated(8).to_a.size}"
    out << "braced: #{Provider.migrated_braced(7).map(&:ledger_account_id).sort == [moved.id]} #{Provider.migrated_braced(7).exists?}"
    out
  end
end
"##;

const EXPECTED: [&str; 2] = ["where.not: true 0", "braced: true true"];

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/ledger_account",
    "app/models/provider",
    "app/models/joins_alias_default_scope_probe",
];

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("roundhouse.yml", ""),
        ("config/database.yml", DATABASE_YML),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", STRUCTURE),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/ledger_account.rb", LEDGER),
        ("app/models/provider.rb", PROVIDER),
        ("app/models/joins_alias_default_scope_probe.rb", PROBE),
    ]
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
fn a_where_keyed_by_the_association_aliases_a_default_scoped_join_like_native_rails_on_postgresql() {
    let files = files();
    let overlay = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "joins-alias-default-scope",
        &files,
        &emitted,
        "rh_spec_joins_alias_default_scope",
        "JoinsAliasDefaultScopeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
