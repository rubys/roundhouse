//! PostgreSQL: relation enumerable where chain, on the Ruby target against native Rails.
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

CREATE TABLE orders (
    id bigserial NOT NULL,
    shop_id bigint,
    total integer,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_relation_enumerable
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :orders
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
  belongs_to :shop
end
"#;

const PROBE: &str = r##"class RelationEnumerableProbe
  def self.names(rel)
    rel.map(&:name).sort.inspect
  end

  def self.lines
    out = []
    model = Shop
    order = Order
    a = Shop.create!(name: "a")
    b = Shop.create!(name: "b")
    Shop.create!(name: "c")
    Order.create!(shop_id: a.id, total: 5)
    Order.create!(shop_id: a.id, total: 7)
    Order.create!(shop_id: b.id, total: 9)
    shops = model.all.order(:id)
    orders = order.all.order(:id)
    out << "filter_map: #{shops.filter_map { |s| s.name.upcase unless s.name == "b" }.inspect}"
    out << "reject: #{shops.reject { |s| s.name == "a" }.map(&:name).inspect}"
    out << "flat_map: #{shops.flat_map { |s| [s.name, s.name] }.inspect}"
    out << "each_with_object: #{shops.each_with_object([]) { |s, acc| acc << s.name }.inspect}"
    out << "each_with_index: #{shops.each_with_index.map { |s, i| "#{i}#{s.name}" }.inspect}"
    out << "min_by: #{orders.min_by { |o| -o.total }.total}"
    out << "sum block: #{orders.sum { |o| o.total }}"
    out << "sum initial: #{orders.sum(100) { |o| o.total }}"
    out << "count block: #{orders.count { |o| o.total > 5 }}"
    out << "find block: #{shops.find { |s| s.name == "b" }.name}"
    out << "select block: #{shops.select { |s| s.name != "a" }.map(&:name).inspect}"
    out << "any block: #{orders.any? { |o| o.total > 8 }} #{orders.any? { |o| o.total > 9 }}"
    out << "none block: #{orders.none? { |o| o.total > 9 }}"
    out << "tally: #{orders.map(&:shop_id).tally.values.sort.inspect}"
    out << "where.not: #{names(model.where.not(name: "b"))}"
    out << "where.missing: #{names(model.where.missing(:orders))}"
    out << "where.associated: #{model.where.associated(:orders).map(&:name).uniq.sort.inspect}"
    out << "[]: #{shops[0].name} #{shops[1..].map(&:name).inspect}"
    out << "reverse: #{shops.reverse.map(&:name).inspect}"
    out << "index: #{shops.index { |s| s.name == "b" }}"
    out << "sample: #{model.where(name: "a").sample.name}"
    out << "each enumerator: #{shops.each.map(&:name).inspect}"
    out << "is enumerable: #{shops.is_a?(Enumerable)}"
    out
  end
end
"##;

const EXPECTED: [&str; 23] = [
    "filter_map: [\"A\", \"C\"]",
    "reject: [\"b\", \"c\"]",
    "flat_map: [\"a\", \"a\", \"b\", \"b\", \"c\", \"c\"]",
    "each_with_object: [\"a\", \"b\", \"c\"]",
    "each_with_index: [\"0a\", \"1b\", \"2c\"]",
    "min_by: 9",
    "sum block: 21",
    "sum initial: 121",
    "count block: 2",
    "find block: b",
    "select block: [\"b\", \"c\"]",
    "any block: true false",
    "none block: true",
    "tally: [1, 2]",
    "where.not: [\"a\", \"c\"]",
    "where.missing: [\"c\"]",
    "where.associated: [\"a\", \"b\"]",
    "[]: a [\"b\", \"c\"]",
    "reverse: [\"c\", \"b\", \"a\"]",
    "index: 1",
    "sample: a",
    "each enumerator: [\"a\", \"b\", \"c\"]",
    "is enumerable: true",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/order.rb", ORDER),
    ("app/models/relation_enumerable_probe.rb", PROBE),
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
fn relation_enumerable_and_where_chain_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation-enumerable",
        &FILES,
        &emitted,
        "rh_spec_relation_enumerable",
        "RelationEnumerableProbe",
        &[
            "app/models/application_record",
            "app/models/shop",
            "app/models/order",
            "app/models/relation_enumerable_probe",
        ],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
