//! PostgreSQL: declared single primary key, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE bodies (
    cityhash bigint NOT NULL,
    island smallint NOT NULL,
    label character varying,
    CONSTRAINT bodies_pkey PRIMARY KEY (cityhash, island)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_declared_single_primary_key
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const BODY: &str = r#"class Body < ApplicationRecord
  self.primary_key = "cityhash"
end
"#;

const PROBE: &str = r##"class DeclaredSinglePrimaryKeyProbe
  def self.lines
    out = []
    out << "key: #{Body.primary_key.inspect} #{Body.composite_primary_key?}"
    a = Body.create!(cityhash: 42, island: 1, label: "a")
    out << "id: #{a.id.inspect} #{a.to_param.inspect} #{Body.find(42).label}"
    b = Body.new(island: 2, label: "b")
    b.id = 43
    b.save!
    out << "id=: #{b.cityhash.inspect} #{Body.order(:cityhash).pluck(:cityhash).inspect}"
    Body.transaction do
      Body.create!(cityhash: 44, island: 1, label: "c")
      raise ActiveRecord::Rollback
    end
    out << "rollback: #{Body.count}"
    a.label = "a2"
    a.save!
    out << "update: #{Body.find(42).label} #{Body.find(43).label}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    r#"key: "cityhash" false"#,
    r#"id: 42 "42" a"#,
    "id=: 43 [42, 43]",
    "rollback: 2",
    "update: a2 b",
];

/// What native Rails requires, in order.
const LOADS: [&str; 3] = [
    "app/models/application_record",
    "app/models/body",
    "app/models/declared_single_primary_key_probe",
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/body.rb", BODY),
    ("app/models/declared_single_primary_key_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: `id` and `id=` are the declared column's.
#[test]
fn declared_single_primary_key_is_the_id() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let body = std::fs::read_to_string(emitted.join("app/models/body.rb")).unwrap();
    for expected in ["def id\n", "@cityhash\n", "def id=(value)\n"] {
        assert!(body.contains(expected), "missing {expected:?} in:\n{body}");
    }
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
fn declared_single_primary_key_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "declared-single-primary-key",
        &FILES,
        &emitted,
        "rh_spec_declared_single_primary_key",
        "DeclaredSinglePrimaryKeyProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
