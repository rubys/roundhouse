//! PostgreSQL: association readers, on the Ruby target against native Rails.
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
    uuid character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
CREATE TABLE items (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    approver_id bigint,
    approver_type character varying,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
CREATE TABLE accounts (
    id bigserial NOT NULL,
    shop_uuid character varying,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_association_readers
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :items
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  belongs_to :shop, optional: true
  belongs_to :approver, polymorphic: true, optional: true, primary_key: :id, foreign_key: :approver_id, inverse_of: false
end
"#;

const ACCOUNT: &str = r#"class Account < ApplicationRecord
  belongs_to :shop, primary_key: :uuid, foreign_key: :shop_uuid
end
"#;

const PROBE: &str = r##"class ReadersProbe
  def self.lines
    out = []
    # A name the compiler cannot read, as a gem's `send` has: the
    # reader's answer gets the call.
    items = [:items].sample
    shop = Shop.create!(name: "s")
    created = shop.send(items).create(title: "a")
    out << "send create: #{created.persisted?} #{created.shop_id == shop.id} #{shop.items.size} #{Item.where(shop_id: shop.id).count}"
    loaded = Shop.find(shop.id)
    loaded.items.to_a
    loaded.send(items).create!(title: "b")
    out << "loaded create!: #{loaded.items.map(&:title).inspect} #{loaded.association(:items).loaded?}"
    out << "first!: #{loaded.items.first!.title}"
    begin
      Shop.create!(name: "e").items.first!
      out << "first! empty: no raise"
    rescue ActiveRecord::RecordNotFound => e
      out << "first! empty: #{e.class.name}"
    end
    fresh = Shop.new(name: "n")
    begin
      fresh.send(items).create(title: "x")
      out << "new owner create: no raise"
    rescue ActiveRecord::RecordNotSaved => e
      out << "new owner create: #{e.message}"
    end
    built = fresh.send(items).build(title: "y")
    out << "new owner build: #{built.new_record?} #{built.shop_id.inspect} #{fresh.items.map(&:title).inspect}"
    out << "where on proxy: #{loaded.send(items).where(title: "b").map(&:title).inspect}"
    out << "sum on proxy: #{loaded.send(items).sum(:id) == Item.where(shop_id: shop.id).map(&:id).sum}"

    item = Item.find(created.id)
    out << "cached before: #{item.association_cached?(:shop)} #{item.association_cached?(:nope)}"
    item.shop
    out << "cached after: #{item.association_cached?(:shop)}"
    owner = Shop.find(shop.id)
    before = owner.association_cached?(:items)
    owner.items.each { |_| nil }
    out << "cached has_many: #{before} #{owner.association_cached?(:items)}"

    out << "approver nil: #{item.approver.inspect}"
    item.update!(approver: shop)
    out << "approver set: #{item.approver_type} #{item.approver_id == shop.id} #{item.approver.equal?(shop)}"
    out << "approver read: #{Item.find(item.id).approver == shop} #{Item.find(item.id).approver.name}"

    gone = Shop.find(shop.id)
    gone.items.to_a
    gone.send(items).destroy_all
    out << "destroy_all: #{gone.items.map(&:title).inspect} #{Item.where(shop_id: shop.id).count}"

    account = Account.new(shop: Shop.new(name: "u", uuid: "u-1"))
    account.save!
    out << "pk autosave: #{account.shop_uuid.inspect} #{account.shop.persisted?} #{Account.find(account.id).shop.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 16] = [
    "send create: true true 1 1",
    r#"loaded create!: ["a", "b"] true"#,
    "first!: a",
    "first! empty: ActiveRecord::RecordNotFound",
    "new owner create: You cannot call create unless the parent is saved",
    r#"new owner build: true nil ["y"]"#,
    r#"where on proxy: ["b"]"#,
    "sum on proxy: true",
    "cached before: false false",
    "cached after: true",
    "cached has_many: false true",
    "approver nil: nil",
    "approver set: Shop true true",
    "approver read: true s",
    "destroy_all: [] 0",
    r#"pk autosave: "u-1" true u"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/shop",
    "app/models/item",
    "app/models/account",
    "app/models/readers_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/item.rb", ITEM),
    ("app/models/account.rb", ACCOUNT),
    ("app/models/readers_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

/// The compiler side: the has_many reader answers the proxy with the
/// reader's scope beside it, every model answers `association_cached?`,
/// the open polymorphic belongs_to has accessors, and the autosave takes
/// the `primary_key:`.
#[test]
#[ignore = "fork spec: postgresql"]
fn association_readers_emit_proxy_cached_polymorphic_and_pk_autosave() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap();
    let shop = read("app/models/shop.rb");
    for expected in [
        "ActiveRecord::Associations::CollectionProxy.wrap(self, :items, ",
        "def _items_association_scope",
        "def association_cached?(name)",
    ] {
        assert!(shop.contains(expected), "missing {expected:?} in:\n{shop}");
    }
    let item = read("app/models/item.rb");
    for expected in ["def approver\n", "Object.const_get(approver_type).find_by(id: approver_id)", "def approver=(value)"] {
        assert!(item.contains(expected), "missing {expected:?} in:\n{item}");
    }
    let account = read("app/models/account.rb");
    assert!(account.contains("@shop_uuid = @shop_cache.uuid"), "autosave ignores primary_key:\n{account}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn association_readers_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "association-readers",
        &FILES,
        &emitted,
        "rh_spec_association_readers",
        "ReadersProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
