//! PostgreSQL: has many extending association owner, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE orders (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);

CREATE TABLE adjustments (
    id bigserial NOT NULL,
    order_id bigint,
    kind character varying,
    CONSTRAINT adjustments_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_has_many_extend_owner\n";

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const EXTENSION: &str = r#"module AdjustmentsExtension
  def owner_name
    @association.owner.name
  end

  def refunds
    where(kind: "refund").where(order_id: @association.owner.id)
  end
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
  has_many :adjustments, -> { order(:id).extending(AdjustmentsExtension) }
end
"#;

const ADJUSTMENT: &str = "class Adjustment < ApplicationRecord\n  belongs_to :order\nend\n";

const PROBE: &str = r##"class HasManyExtendOwnerProbe
  def self.lines
    out = []
    a = Order.create!(name: "a")
    b = Order.create!(name: "b")
    Adjustment.create!(order_id: a.id, kind: "refund")
    Adjustment.create!(order_id: a.id, kind: "fee")
    Adjustment.create!(order_id: b.id, kind: "refund")
    order = Order.find(a.id)
    out << "association: #{order.association(:adjustments).class.name}"
    out << "owner_name: #{order.adjustments.owner_name}"
    out << "refunds: #{order.adjustments.refunds.map(&:kind).inspect} #{Order.find(b.id).adjustments.refunds.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 3] = ["association: ActiveRecord::Associations::HasManyAssociation", "owner_name: a", "refunds: [\"refund\"] 1"];

const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/adjustments_extension",
    "app/models/order",
    "app/models/adjustment",
    "app/models/has_many_extend_owner_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/adjustments_extension.rb", EXTENSION),
    ("app/models/order.rb", ORDER),
    ("app/models/adjustment.rb", ADJUSTMENT),
    ("app/models/has_many_extend_owner_probe.rb", PROBE),
];

#[test]
#[ignore = "fork spec: postgresql"]
fn has_many_extension_reads_the_association_owner_on_postgresql() {
    let app = FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, _errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "has-many-extend-owner",
        &FILES,
        &emitted,
        "rh_spec_has_many_extend_owner",
        "HasManyExtendOwnerProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
