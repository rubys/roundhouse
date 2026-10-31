//! PostgreSQL: after all transactions commit, on the Ruby target against native Rails.
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
    CONSTRAINT things_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_after_all_txn_commit
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const THING: &str = r#"class Thing < ApplicationRecord
end
"#;

const PROBE: &str = r#"class AfterAllProbe
  def self.lines
    out = []
    log = []
    ActiveRecord.after_all_transactions_commit { log << "outside" }
    out << "no transaction: #{log.join(",")}"

    log = []
    Thing.transaction do
      ActiveRecord.after_all_transactions_commit { log << "inside" }
      log << "body"
    end
    out << "joinable: #{log.join(",")}"

    log = []
    Thing.transaction do
      Thing.transaction(requires_new: true) do
        ActiveRecord.after_all_transactions_commit { log << "nested" }
        log << "inner"
      end
      log << "outer"
    end
    out << "savepoint: #{log.join(",")}"

    log = []
    Thing.transaction do
      Thing.transaction(requires_new: true) do
        ActiveRecord.after_all_transactions_commit { log << "released" }
      end
      log << "after-release"
    end
    out << "released savepoint: #{log.join(",")}"

    log = []
    Thing.transaction do
      ActiveRecord.after_all_transactions_commit { log << "rolled-back" }
      raise ActiveRecord::Rollback
    end
    out << "rollback: [#{log.join(",")}]"

    log = []
    Thing.transaction do
      Thing.transaction(requires_new: true) do
        ActiveRecord.after_all_transactions_commit { log << "savepoint-rollback" }
        raise ActiveRecord::Rollback
      end
      log << "outer-commits"
    end
    out << "savepoint rollback: #{log.join(",")}"

    log = []
    Thing.transaction do
      ActiveRecord.after_all_transactions_commit { log << "a" }
      ActiveRecord.after_all_transactions_commit { log << "b" }
    end
    out << "order: #{log.join(",")}"

    log = []
    Thing.transaction do
      Thing.create!(name: "x")
      ActiveRecord.after_all_transactions_commit { log << "count=#{Thing.count}" }
    end
    out << "sees writes: #{log.join(",")}"

    log = []
    Thing.connection.begin_transaction(joinable: false)
    begin
      ActiveRecord.after_all_transactions_commit { log << "wrapper-ignored" }
      log << "wrapped"
      Thing.transaction do
        ActiveRecord.after_all_transactions_commit { log << "inner" }
        log << "body"
      end
      log << "after-inner"
    ensure
      Thing.connection.rollback_transaction
    end
    out << "non-joinable wrapper: #{log.join(",")}"

    out
  end
end
"#;

const EXPECTED: [&str; 9] = [
    "no transaction: outside",
    "joinable: body,inside",
    "savepoint: inner,outer,nested",
    "released savepoint: after-release,released",
    "rollback: []",
    "savepoint rollback: outer-commits",
    "order: a,b",
    "sees writes: count=1",
    "non-joinable wrapper: wrapper-ignored,wrapped,body,inner,after-inner",
];

const LOADS: [&str; 3] = [
    "app/models/application_record",
    "app/models/thing",
    "app/models/after_all_probe",
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/thing.rb", THING),
    ("app/models/after_all_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn after_all_transactions_commit_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{real:?}");
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "after-all-transactions-commit",
        &FILES,
        &emitted,
        "rh_spec_after_all_txn_commit",
        "AfterAllProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
