//! PostgreSQL: association owner scope, on the Ruby target against native Rails.
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
    region character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
CREATE TABLE items (
    id bigserial NOT NULL,
    shop_id bigint,
    region character varying,
    title character varying,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_owner_scope
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :regional_items, ->(shop) { where(region: shop.region).order("title DESC") },
           class_name: "Item", foreign_key: :shop_id
  has_one :regional_item, ->(owner) { where(region: owner.region).order(:title) },
          class_name: "Item", foreign_key: :shop_id
  has_many :all_items, class_name: "Item", foreign_key: :shop_id
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  belongs_to :shop, optional: true
end
"#;

const PROBE: &str = r##"class OwnerScopeProbe
  def self.lines
    out = []
    eu = Shop.create!(region: "eu")
    us = Shop.create!(region: "us")
    Item.create!(shop_id: eu.id, region: "eu", title: "a")
    Item.create!(shop_id: eu.id, region: "us", title: "b")
    Item.create!(shop_id: eu.id, region: "eu", title: "c")
    Item.create!(shop_id: us.id, region: "us", title: "d")
    Item.create!(shop_id: us.id, region: "eu", title: "e")
    owner = Shop.find(eu.id)
    other = Shop.find(us.id)

    out << "reader: #{owner.regional_items.map(&:title).inspect}"
    out << "reader other: #{other.regional_items.map(&:title).inspect}"
    out << "unscoped: #{owner.all_items.map(&:title).sort.inspect}"
    out << "count: #{Shop.find(eu.id).regional_items.count}"
    out << "exists: #{Shop.find(eu.id).regional_items.exists?(title: "b")} #{Shop.find(eu.id).regional_items.exists?(title: "c")}"
    out << "where chain: #{Shop.find(eu.id).regional_items.where(title: "c").map(&:id).size}"
    built = Shop.find(eu.id).regional_items.build
    out << "build: #{built.region.inspect} #{built.shop_id == eu.id}"
    out << "has_one: #{Shop.find(eu.id).regional_item&.title.inspect}"
    out << "has_one other: #{Shop.find(us.id).regional_item&.title.inspect}"
    built_one = Shop.find(eu.id).build_regional_item
    out << "build_has_one: #{built_one.region.inspect} #{built_one.shop_id == eu.id}"
    out << "new owner: #{Shop.new(region: "eu").regional_items.to_a.inspect}"
    # Rails refuses to eager load or join an instance-dependent scope
    # (reflection.rb `check_eager_loadable!`).
    begin
      Shop.eager_load(:regional_items).to_a
      out << "eager_load: ok"
    rescue ArgumentError
      out << "eager_load: ArgumentError"
    end
    begin
      Shop.joins(:regional_items).to_a
      out << "joins: ok"
    rescue ArgumentError
      out << "joins: ArgumentError"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 13] = [
    r#"reader: ["c", "a"]"#,
    r#"reader other: ["d"]"#,
    r#"unscoped: ["a", "b", "c"]"#,
    "count: 2",
    "exists: false true",
    "where chain: 1",
    r#"build: "eu" true"#,
    r#"has_one: "a""#,
    r#"has_one other: "d""#,
    r#"build_has_one: "eu" true"#,
    "new owner: []",
    "eager_load: ArgumentError",
    "joins: ArgumentError",
];

/// What native Rails requires, in order.
const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/shop",
    "app/models/item",
    "app/models/owner_scope_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/item.rb", ITEM),
    ("app/models/owner_scope_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: both readers carry the scope's conditions with the
/// owner bound to `self`, rather than the foreign-key query alone.
#[test]
#[ignore = "fork spec: postgresql"]
fn owner_taking_scope_is_grafted_onto_the_reader() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shop = std::fs::read_to_string(emitted.join("app/models/shop.rb")).unwrap();
    for expected in ["def regional_items", "def regional_item"] {
        assert!(shop.contains(expected), "missing {expected:?} in:\n{shop}");
    }
    assert!(shop.contains("title DESC"), "the scope's ORDER is dropped:\n{shop}");
    assert!(shop.contains("region"), "the scope's condition is dropped:\n{shop}");
    assert!(!shop.contains("shop.region") && !shop.contains("owner.region"), "the owner parameter is unbound:\n{shop}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn owner_taking_scope_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "owner-scope",
        &FILES,
        &emitted,
        "rh_spec_owner_scope",
        "OwnerScopeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
