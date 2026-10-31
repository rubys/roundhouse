//! PostgreSQL: relation runtime association joins, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);

CREATE TABLE owners (
    id bigserial NOT NULL,
    shop_id bigint,
    name character varying,
    CONSTRAINT owners_pkey PRIMARY KEY (id)
);

CREATE TABLE orders (
    id bigserial NOT NULL,
    shop_id bigint,
    total integer,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);

CREATE TABLE lines (
    id bigserial NOT NULL,
    order_id bigint,
    sku character varying,
    CONSTRAINT lines_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_relation_runtime_joins
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :orders
  has_many :lines, through: :orders
  has_one :owner
end
"#;

const OWNER: &str = r#"class Owner < ApplicationRecord
  belongs_to :shop
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
  belongs_to :shop
  has_many :lines
end
"#;

const LINE: &str = r#"class Line < ApplicationRecord
  belongs_to :order
end
"#;

/// `model` and `spec` are parameters: the receiver's class is not known
/// where the join is written, so nothing expands at transpile time.
const PROBE: &str = r##"class RelationRuntimeJoinsProbe
  def self.inner(model, spec)
    model.all.joins(spec).to_a.map(&:id).sort.inspect
  end

  def self.outer(model, spec)
    model.all.left_joins(spec).to_a.map(&:id).sort.inspect
  end

  def self.refused(model, spec)
    model.all.joins(spec).to_a
    "joined"
  rescue StandardError
    "raised"
  end

  def self.lines
    out = []
    a = Shop.create!(name: "a")
    b = Shop.create!(name: "b")
    c = Shop.create!(name: "c")
    o1 = Order.create!(shop_id: a.id, total: 5)
    o2 = Order.create!(shop_id: a.id, total: 7)
    o3 = Order.create!(shop_id: b.id, total: 9)
    Line.create!(order_id: o1.id, sku: "x")
    Line.create!(order_id: o1.id, sku: "y")
    Line.create!(order_id: o3.id, sku: "z")
    Owner.create!(shop_id: a.id, name: "ann")
    Owner.create!(shop_id: c.id, name: "cy")
    out << "ids: #{[a.id, b.id, c.id, o1.id, o2.id, o3.id].inspect == [a.id, b.id, c.id, o1.id, o2.id, o3.id].inspect}"
    out << "has_many inner: #{inner(Shop, :orders).then { |s| s.scan(/\d+/).map(&:to_i).tally.values.sort.inspect }}"
    out << "has_many left: #{outer(Shop, :orders).scan(/\d+/).size}"
    out << "belongs_to inner: #{inner(Order, :shop).scan(/\d+/).size}"
    out << "has_one inner: #{inner(Shop, :owner).scan(/\d+/).size}"
    out << "has_one left unmatched: #{Shop.all.left_joins(:owner).where("owners.id IS NULL").to_a.map(&:name).inspect}"
    out << "nested inner: #{inner(Shop, { orders: :lines }).scan(/\d+/).size}"
    out << "nested left: #{outer(Shop, { orders: :lines }).scan(/\d+/).size}"
    out << "through inner: #{inner(Shop, :lines).scan(/\d+/).size}"
    out << "through left: #{outer(Shop, :lines).scan(/\d+/).size}"
    out << "array: #{inner(Shop, [:orders, :owner]).scan(/\d+/).size}"
    out << "filtered: #{Shop.all.joins(:orders).where("orders.total > 5").to_a.map(&:name).sort.inspect}"
    out << "missing: #{refused(Shop, :nothing)}"
    out
  end
end
"##;

const EXPECTED: [&str; 13] = [
    "ids: true",
    "has_many inner: [1, 2]",
    "has_many left: 4",
    "belongs_to inner: 3",
    "has_one inner: 2",
    "has_one left unmatched: [\"b\"]",
    "nested inner: 3",
    "nested left: 5",
    "through inner: 3",
    "through left: 5",
    "array: 2",
    "filtered: [\"a\", \"b\"]",
    "missing: raised",
];

const FILES: [(&str, &str); 12] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/owner.rb", OWNER),
    ("app/models/order.rb", ORDER),
    ("app/models/line.rb", LINE),
    ("app/models/relation_runtime_joins_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn runtime_association_joins_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation-runtime-joins",
        &FILES,
        &emitted,
        "rh_spec_relation_runtime_joins",
        "RelationRuntimeJoinsProbe",
        &[
            "app/models/application_record",
            "app/models/shop",
            "app/models/owner",
            "app/models/order",
            "app/models/line",
            "app/models/relation_runtime_joins_probe",
        ],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
