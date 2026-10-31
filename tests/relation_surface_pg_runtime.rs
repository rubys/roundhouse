//! PostgreSQL: relation surface, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE zones (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT zones_pkey PRIMARY KEY (id)
);
CREATE TABLE provinces (
    id bigserial NOT NULL,
    code character varying,
    CONSTRAINT provinces_pkey PRIMARY KEY (id)
);
CREATE TABLE zone_regions (
    id bigserial NOT NULL,
    zone_id bigint,
    region_id bigint,
    region_type character varying,
    CONSTRAINT zone_regions_pkey PRIMARY KEY (id)
);
CREATE TABLE gadgets (
    id bigserial NOT NULL,
    island bigint NOT NULL DEFAULT 0,
    gift boolean NOT NULL DEFAULT false,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);
CREATE TABLE gadget_variants (
    id bigserial NOT NULL,
    island bigint NOT NULL DEFAULT 0,
    gadget_id bigint NOT NULL,
    sku character varying,
    CONSTRAINT gadget_variants_pkey PRIMARY KEY (id)
);
CREATE TABLE gadget_stocks (
    id bigserial NOT NULL,
    island bigint NOT NULL DEFAULT 0,
    gadget_variant_id bigint NOT NULL,
    stock_id bigint NOT NULL,
    CONSTRAINT gadget_stocks_pkey PRIMARY KEY (id)
);
CREATE TABLE gadget_tags (
    id bigserial NOT NULL,
    gadget_id bigint NOT NULL,
    name character varying,
    CONSTRAINT gadget_tags_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_relation
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const DELIVERY_PROVINCE: &str = "module Delivery\n  Province = Struct.new(:id, :name, :code)\nend\n";
const PROVINCE: &str = "class Province < ApplicationRecord\nend\n";
const DELIVERY_ZONE: &str = r#"module Delivery
  class Zone < ApplicationRecord
    self.table_name = "zones"
    has_many :zone_regions, class_name: "Delivery::ZoneRegion", foreign_key: "zone_id"
    has_many :provinces, through: :zone_regions, source: :region, source_type: "Province", class_name: "::Province"
  end
end
"#;
const DELIVERY_ZONE_REGION: &str = r#"module Delivery
  class ZoneRegion < ApplicationRecord
    self.table_name = "zone_regions"
    belongs_to :zone, class_name: "Delivery::Zone"
    belongs_to :region, polymorphic: true
  end
end
"#;

const GADGET: &str = r#"class Gadget < ApplicationRecord
  has_many :variants, class_name: "GadgetVariant"
  has_many :tags, class_name: "GadgetTag"

  def self.stocked_at(stock_id)
    joins(variants: :gadget_stock).where(variants: { gadget_stocks: { stock_id: stock_id } })
  end

  def self.tagged(name)
    joins("INNER JOIN gadget_tags ON gadget_tags.gadget_id = gadgets.id").where(gadget_tags: { name: name })
  end
end
"#;
const GADGET_VARIANT: &str = r#"class GadgetVariant < ApplicationRecord
  default_scope { where(island: 0) }
  belongs_to :gadget
  has_one :gadget_stock, class_name: "GadgetStock"
end
"#;
const GADGET_STOCK: &str = r#"class GadgetStock < ApplicationRecord
  default_scope { where(island: 0) }
  belongs_to :gadget_variant
end
"#;
const GADGET_TAG: &str = "class GadgetTag < ApplicationRecord\n  belongs_to :gadget\nend\n";

const PROBE: &str = r##"class SurfaceRelationProbe
  def self.lines
    out = []
    zone = Delivery::Zone.create!(name: "z")
    ontario = Province.create!(code: "ON")
    Province.create!(code: "QC")
    Delivery::ZoneRegion.create!(zone: zone, region: ontario)
    out << "through: #{Delivery::Zone.find(zone.id).provinces.map(&:code).inspect} #{Delivery::Zone.find(zone.id).provinces.first.class.name}"

    plain = Gadget.create!(gift: false)
    gift = Gadget.create!(gift: true)
    [[plain, "a", 1], [gift, "b", 2]].each do |gadget, sku, stock|
      variant = GadgetVariant.create!(gadget_id: gadget.id, sku: sku)
      GadgetStock.create!(gadget_variant_id: variant.id, stock_id: stock)
    end
    GadgetTag.create!(gadget_id: gift.id, name: "x")
    out << "nested: #{Gadget.stocked_at(2).pluck(:id) == [gift.id]} #{Gadget.stocked_at(3).exists?}"
    out << "nested gift: #{Gadget.where(gift: true).stocked_at(2).exists?} #{Gadget.where(gift: false).stocked_at(2).exists?}"
    merged = Gadget.includes(:variants).joins(:variants).where(variants: { sku: "b" }).merge(Gadget.tagged("x"))
    out << "merge: #{merged.map(&:id) == [gift.id]} #{Gadget.includes(:variants).joins(:variants).where(variants: { sku: "a" }).merge(Gadget.tagged("x")).to_a.size}"
    out << "merge plain: #{Gadget.all.merge(Gadget.tagged("x")).pluck(:id) == [gift.id]}"
    out << "reorder: #{Gadget.order(:id).reorder(nil).count} #{Gadget.reorder(nil).pluck(:id).sort == [plain.id, gift.id]} #{Gadget.order(nil).count}"
    out << "scope attributes: #{Gadget.scope_attributes.transform_keys(&:to_s).inspect} #{Gadget.where(gift: true).scope_attributes.transform_keys(&:to_s).inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 7] = [
    "through: [\"ON\"] Province",
    "nested: true false",
    "nested gift: true false",
    "merge: true 0",
    "merge plain: true",
    "reorder: 2 true 2",
    "scope attributes: {} {\"gift\" => true}",
];

const LOADS: [&str; 10] = [
    "app/models/application_record",
    "app/models/delivery/province",
    "app/models/province",
    "app/models/delivery/zone",
    "app/models/delivery/zone_region",
    "app/models/gadget",
    "app/models/gadget_variant",
    "app/models/gadget_stock",
    "app/models/gadget_tag",
    "app/models/surface_relation_probe",
];

const FILES: [(&str, &str); 15] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/delivery/province.rb", DELIVERY_PROVINCE),
    ("app/models/province.rb", PROVINCE),
    ("app/models/delivery/zone.rb", DELIVERY_ZONE),
    ("app/models/delivery/zone_region.rb", DELIVERY_ZONE_REGION),
    ("app/models/gadget.rb", GADGET),
    ("app/models/gadget_variant.rb", GADGET_VARIANT),
    ("app/models/gadget_stock.rb", GADGET_STOCK),
    ("app/models/gadget_tag.rb", GADGET_TAG),
    ("app/models/surface_relation_probe.rb", PROBE),
    ("test/.keep", ""),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The through reader's target is written rooted, without a database.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_through_reader_roots_a_top_level_target_a_namespace_shadows() {
    let (emitted, _errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let zone = std::fs::read_to_string(emitted.join("app/models/delivery/zone.rb")).expect("delivery/zone.rb emitted");
    assert!(zone.contains("ActiveRecord::Relation.new(::Province)"), "{zone}");
    assert!(!zone.contains("Relation.new(Province)"), "{zone}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn relation_paths_surface_answer_like_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "surface-relation",
        &FILES,
        &emitted,
        "rh_spec_relation",
        "SurfaceRelationProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
