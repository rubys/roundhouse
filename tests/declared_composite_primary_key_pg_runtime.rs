//! PostgreSQL: declared composite primary key, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE pins (
    shop_id bigint NOT NULL,
    user_id bigint NOT NULL,
    label character varying,
    CONSTRAINT pins_pkey PRIMARY KEY (shop_id, user_id)
);

CREATE TABLE invoices (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    number character varying,
    CONSTRAINT invoices_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_declared_composite_primary_key
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const PIN: &str = r#"class Pin < ApplicationRecord
  self.primary_key = [:user_id, :shop_id]
end
"#;

const INVOICE: &str = r#"class Invoice < ApplicationRecord
  self.primary_key = ["shop_id", "id"]
end
"#;

const PROBE: &str = r##"class DeclaredCompositePrimaryKeyProbe
  def self.lines
    out = []
    out << "key: #{Pin.primary_key.inspect} #{Pin.composite_primary_key?} #{Invoice.primary_key.inspect} #{Invoice.composite_primary_key?}"
    a = Pin.create!(user_id: 7, shop_id: 1, label: "a")
    b = Pin.create!(user_id: 7, shop_id: 2, label: "b")
    out << "pin id: #{a.id.inspect} #{a.to_param.inspect} #{Pin.find([7, 2]).label} #{Pin.ids.sort.inspect}"
    a.label = "a2"
    a.save!
    out << "pin update: #{Pin.order(:shop_id).pluck(:label).inspect}"
    b.destroy
    out << "pin destroy: #{Pin.count} #{Pin.first.label}"
    i = Invoice.create!(shop_id: 3, number: "n1")
    j = Invoice.create!(shop_id: 4, number: "n2")
    out << "invoice id: #{i.id == [3, i.id_value]} #{i.id_value.is_a?(Integer)} #{Invoice.find([4, j.id_value]).number}"
    j.number = "n3"
    j.save!
    out << "invoice update: #{Invoice.order(:shop_id).pluck(:number).inspect} #{j.reload.number}"
    i.destroy
    out << "invoice destroy: #{Invoice.count} #{Invoice.first.number}"
    out
  end
end
"##;

const EXPECTED: [&str; 7] = [
    r#"key: ["user_id", "shop_id"] true ["shop_id", "id"] true"#,
    r#"pin id: [7, 1] "7_1" b [[7, 1], [7, 2]]"#,
    r#"pin update: ["a2", "b"]"#,
    "pin destroy: 1 a2",
    "invoice id: true true n2",
    r#"invoice update: ["n1", "n3"] n3"#,
    "invoice destroy: 1 n3",
];

/// What native Rails requires, in order.
const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/pin",
    "app/models/invoice",
    "app/models/declared_composite_primary_key_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/pin.rb", PIN),
    ("app/models/invoice.rb", INVOICE),
    ("app/models/declared_composite_primary_key_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the declaration is consumed into the key, in the
/// order written.
#[test]
#[ignore = "fork spec: postgresql"]
fn declared_composite_primary_key_compiles_to_the_tuple() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let pin = std::fs::read_to_string(emitted.join("app/models/pin.rb")).unwrap();
    for expected in [
        "  def id\n    [self.user_id, self.shop_id]\n  end\n",
        "  def self.primary_key\n    [\"user_id\", \"shop_id\"]\n  end\n",
    ] {
        assert!(pin.contains(expected), "missing {expected:?} in:\n{pin}");
    }
    let invoice = std::fs::read_to_string(emitted.join("app/models/invoice.rb")).unwrap();
    // In the model's GeneratedAttributeMethods module.
    assert!(invoice.contains("    def id\n      [self.shop_id, self.id_value]\n    end\n"), "invoice:\n{invoice}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn declared_composite_primary_key_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "declared-composite-primary-key",
        &FILES,
        &emitted,
        "rh_spec_declared_composite_primary_key",
        "DeclaredCompositePrimaryKeyProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
