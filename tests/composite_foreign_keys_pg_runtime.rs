//! PostgreSQL: composite foreign keys, on the Ruby target against native Rails.
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
    shop_id bigint NOT NULL,
    name character varying,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);

CREATE TABLE draft_orders (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    active_order_id bigint,
    title character varying,
    CONSTRAINT draft_orders_pkey PRIMARY KEY (id)
);

CREATE TABLE order_lines (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    order_id bigint,
    sku character varying,
    CONSTRAINT order_lines_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_composite_fk
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
  has_one :draft_order,
    inverse_of: :active_order,
    foreign_key: [:active_order_id, :shop_id],
    primary_key: [:id, :shop_id]
  has_many :order_lines, foreign_key: [:shop_id, :order_id], primary_key: [:shop_id, :id]
end
"#;

const DRAFT_ORDER: &str = r#"class DraftOrder < ApplicationRecord
  belongs_to :active_order, class_name: "Order", optional: true, inverse_of: :draft_order,
    foreign_key: [:active_order_id, :shop_id],
    primary_key: [:id, :shop_id]
end
"#;

const ORDER_LINE: &str = r#"class OrderLine < ApplicationRecord
  belongs_to :order, foreign_key: [:shop_id, :order_id], primary_key: [:shop_id, :id]
end
"#;

const PROBE: &str = r##"class CompositeForeignKeysProbe
  def self.lines
    out = []
    o1 = Order.create!(shop_id: 1, name: "one")
    o2 = Order.create!(shop_id: 2, name: "two")
    d1 = DraftOrder.create!(shop_id: 1, active_order_id: o1.id, title: "d1")
    dx = DraftOrder.create!(shop_id: 2, active_order_id: o1.id, title: "other shop")
    l1 = OrderLine.create!(shop_id: 1, order_id: o1.id, sku: "a")
    lx = OrderLine.create!(shop_id: 2, order_id: o1.id, sku: "x")
    OrderLine.create!(shop_id: 2, order_id: o2.id, sku: "b")

    out << "has_one: #{Order.find(o1.id).draft_order&.title.inspect} #{Order.find(o2.id).draft_order.inspect}"
    out << "has_many: #{Order.find(o1.id).order_lines.map(&:sku).sort.inspect} #{Order.find(o2.id).order_lines.map(&:sku).inspect}"
    out << "belongs_to: #{DraftOrder.find(d1.id).active_order&.name.inspect} #{DraftOrder.find(dx.id).active_order.inspect}"
    out << "belongs_to plain: #{OrderLine.find(l1.id).order&.name.inspect} #{OrderLine.find(lx.id).order.inspect}"
    out << "joins has_many: #{Order.joins(:order_lines).where(order_lines: { sku: "a" }).count} #{Order.joins(:order_lines).where(order_lines: { sku: "x" }).count}"
    out << "joins has_one: #{Order.joins(:draft_order).where(draft_orders: { title: "d1" }).count} #{Order.joins(:draft_order).where(draft_orders: { title: "other shop" }).count}"
    out << "joins belongs_to: #{OrderLine.joins(:order).where(orders: { name: "one" }).pluck(:sku).sort.inspect}"
    out << "includes: #{Order.includes(:order_lines, :draft_order).order(:id).map { |o| [o.order_lines.map(&:sku), o.draft_order&.title] }.inspect}"
    out << "where association: #{OrderLine.where(order: Order.find(o1.id)).pluck(:sku).inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 9] = [
    "has_one: \"d1\" nil",
    "has_many: [\"a\"] [\"b\"]",
    "belongs_to: \"one\" nil",
    "belongs_to plain: \"one\" nil",
    "joins has_many: 1 0",
    "joins has_one: 1 0",
    "joins belongs_to: [\"a\"]",
    "includes: [[[\"a\"], \"d1\"], [[\"b\"], nil]]",
    "where association: [\"a\"]",
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/order",
    "app/models/draft_order",
    "app/models/order_line",
    "app/models/composite_foreign_keys_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/order.rb", ORDER),
    ("app/models/draft_order.rb", DRAFT_ORDER),
    ("app/models/order_line.rb", ORDER_LINE),
    ("app/models/composite_foreign_keys_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn composite_foreign_keys_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "composite-foreign-keys",
        &FILES,
        &emitted,
        "rh_spec_composite_fk",
        "CompositeForeignKeysProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
