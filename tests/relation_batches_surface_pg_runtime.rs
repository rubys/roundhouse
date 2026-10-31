//! PostgreSQL: relation batches surface, on the Ruby target against native Rails.
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

CREATE TABLE items (
    id bigserial NOT NULL,
    shop_id bigint,
    qty integer,
    label character varying,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_probe_r4_relation_surface
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ITEM_DEFAULTS: &str = r#"module ItemDefaults
  def with_defaults
    to_a.map(&:label).sort + ["default"]
  end
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :items, extend: ItemDefaults
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  belongs_to :shop
end
"#;

const PROBE: &str = r##"class RelationBatchesProbe
  def self.lines
    out = []
    a = Shop.create!(name: "a")
    b = Shop.create!(name: "b")
    5.times { |i| Item.create!(shop_id: a.id, qty: i.even? ? i : nil, label: "l#{i}") }
    Item.create!(shop_id: b.id, qty: 9, label: "x")
    mine = Item.where(shop_id: a.id)
    ids = Item.order(:id).ids
    labels = ->(rel) { rel.order(:id).pluck(:label) }

    out << "count column: #{mine.count(:qty)} #{mine.count(:id)} #{mine.count(:all)} #{mine.count}"
    out << "count distinct: #{mine.distinct.count(:qty)}"
    out << "count block: #{mine.count { |i| i.qty.nil? }}"
    out << "count none: #{mine.none.count(:qty)}"

    out << "invert one: #{labels.call(mine.invert_where)}"
    out << "invert two: #{labels.call(Item.where(shop_id: a.id, label: "l0").invert_where)}"
    out << "invert keeps order: #{Item.where(shop_id: a.id).order(id: :desc).invert_where.map(&:label).inspect}"

    out << "except where: #{Item.where(shop_id: a.id).except(:where).count}"
    out << "except order: #{Item.order(:id).limit(2).except(:limit).count}"
    loaded = Item.where(shop_id: a.id).load
    out << "reset: #{!!loaded.loaded?} #{!!loaded.reset.loaded?}"

    out << "enum: #{Item.find_each(batch_size: 2).class} #{Item.find_each(batch_size: 2).size} #{Item.find_each(batch_size: 2).map(&:label).inspect}"
    out << "find_each: #{Item.find_each(batch_size: 4).map(&:label).inspect}"
    seen = []
    Item.find_each(batch_size: 2) { |i| seen << i.label }
    out << "find_each block: #{seen.inspect}"
    out << "find_each desc: #{Item.find_each(batch_size: 4, order: :desc).map(&:label).inspect}"
    out << "find_each bounds: #{Item.find_each(start: ids[1], finish: ids[3], batch_size: 2).map(&:label).inspect}"
    out << "find_each limit: #{Item.limit(3).find_each(batch_size: 2).map(&:label).inspect}"
    out << "find_each scope: #{mine.find_each(batch_size: 2).map(&:label).inspect}"
    out << "find_in_batches: #{Item.find_in_batches(batch_size: 4).map(&:size).inspect}"
    out << "find_in_batches size: #{Item.find_in_batches(batch_size: 4).size}"
    sizes = []
    Item.in_batches(of: 4) { |rel| sizes << [rel.class.name, rel.count] }
    out << "in_batches: #{sizes.inspect}"
    ranged = []
    Item.in_batches(of: 4, use_ranges: true) { |rel| ranged << labels.call(rel) }
    out << "in_batches ranges: #{ranged.inspect}"
    loaded_batches = []
    Item.in_batches(of: 4, load: true) { |rel| loaded_batches << [rel.loaded?, rel.map(&:label).sort] }
    out << "in_batches load: #{loaded_batches.inspect}"
    out << "in_batches enum: #{Item.in_batches(of: 4).class} #{Item.in_batches(of: 4).map { |rel| rel.count }.inspect}"
    out << "in_batches each_record: #{Item.in_batches(of: 4).each_record.map(&:label).inspect}"
    out << "in_batches cursor: #{Item.in_batches(of: 4, cursor: [:shop_id, :id]).map { |rel| labels.call(rel) }.inspect}"
    out << "in_batches enumerable: #{Item.in_batches(of: 2).each_with_index.map { |rel, i| "#{i}:#{rel.count}" }.inspect}"
    out << "class side: #{Shop.find_each.map(&:name).inspect} #{Shop.find_in_batches.map(&:size).inspect} #{Shop.in_batches(of: 1).map(&:count).inspect}"
    out << "bad order: #{begin; Item.find_each(order: :sideways) { }; rescue ArgumentError => e; e.class.name; end}"

    out << "proxy count: #{a.items.count(:qty)} #{a.items.count}"
    out << "proxy extend: #{a.items.with_defaults.inspect}"
    out << "proxy find_each: #{a.items.find_each(batch_size: 2).map(&:label).inspect}"

    out << "update!: #{mine.where(label: "l1").update!(label: "one").map(&:label).inspect} #{Item.find_by(label: "one").qty.inspect}"
    out << "first_or_create!: #{Item.where(label: "fresh").first_or_create!(shop_id: b.id).label} #{Item.where(label: "fresh").first_or_create!(shop_id: b.id).id == Item.where(label: "fresh").first.id} #{Item.count}"
    out << "delete_all in batches: #{Item.where(label: "x").in_batches.delete_all} #{Item.where(label: "x").count}"
    out << "update_all in batches: #{Item.in_batches(of: 3).update_all(qty: 1)} #{Item.distinct.pluck(:qty).inspect}"
    out
  end
end
"##;

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/item_defaults.rb", ITEM_DEFAULTS),
    ("app/models/shop.rb", SHOP),
    ("app/models/item.rb", ITEM),
    ("app/models/relation_batches_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The shipped runtime defines the surface (no database needed).
#[test]
#[ignore = "fork spec: postgresql"]
fn the_cruby_overlay_defines_the_batch_and_relation_surface() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let ext = std::fs::read_to_string(emitted.join("runtime/active_record_relation_ext.rb")).unwrap();
    for needle in [
        "module RelationBatches",
        "def in_batches(of: 1000, start: nil, finish: nil, load: false, error_on_ignore: nil, order: DEFAULT_ORDER,",
        "class BatchEnumerator",
        "def invert_where",
        "def count(column_name = nil, &block)",
        "Base.singleton_class.prepend(BaseBatches)",
    ] {
        assert!(ext.contains(needle), "missing {needle}");
    }
    let associations = std::fs::read_to_string(emitted.join("runtime/active_record_associations.rb")).unwrap();
    assert!(associations.contains("reflection.options[:extend]"), "the association's extend: modules are applied");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn relation_batches_and_surface_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation-batches-surface",
        &FILES,
        &emitted,
        "rh_spec_probe_r4_relation_surface_t",
        "RelationBatchesProbe",
        &[
            "app/models/application_record",
            "app/models/item_defaults",
            "app/models/shop",
            "app/models/item",
            "app/models/relation_batches_probe",
        ],
    ) else { return };
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted vs native Rails");
    // Spot-check the oracle itself so two identical wrong answers still fail.
    assert_eq!(native[0], "count column: 3 5 5 5");
    assert_eq!(native[1], "count distinct: 3");
    assert!(native.iter().any(|l| l == "find_in_batches: [4, 2]"), "{native:?}");
}
