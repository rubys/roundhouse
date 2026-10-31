//! PostgreSQL: inheritance column, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE conditions (
    id bigserial NOT NULL,
    attribute_name character varying,
    type character varying,
    value character varying,
    CONSTRAINT conditions_pkey PRIMARY KEY (id)
);

CREATE TABLE assets (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    CONSTRAINT assets_pkey PRIMARY KEY (id)
);

CREATE TABLE tiles (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    CONSTRAINT tiles_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_inheritance_column
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const CONDITION: &str = r#"class Condition < ApplicationRecord
  self.inheritance_column = "attribute_name"
end
"#;

const TITLE_CONDITION: &str = r#"class TitleCondition < Condition
end
"#;

const PRICE_CONDITION: &str = r#"class PriceCondition < Condition
  validates :value, uniqueness: true
end
"#;

const ASSET: &str = r#"class Asset < ApplicationRecord
  self.inheritance_column = nil
end
"#;

const BIG_ASSET: &str = r#"class BigAsset < Asset
  def label
    "big #{name}"
  end
end
"#;

const TILE: &str = r#"class Tile < ApplicationRecord
  self.inheritance_column = :_type_disabled
end
"#;

const WIDE_TILE: &str = r#"class WideTile < Tile
end
"#;

const PROBE: &str = r##"class InheritanceColumnProbe
  def self.lines
    out = []
    TitleCondition.create!(value: "t")
    PriceCondition.create!(value: "p", type: "data")
    TitleCondition.create!(value: "u")
    first = Condition.order(:id).first
    out << "custom: #{TitleCondition.count} #{PriceCondition.count} #{Condition.count} #{TitleCondition.first.attribute_name} #{PriceCondition.first.type} #{TitleCondition.new.attribute_name} #{Condition.inheritance_column} #{TitleCondition.inheritance_column}"
    out << "hydrate: #{Condition.order(:id).map { |c| c.class.name }.join(",")} #{first.is_a?(TitleCondition)} #{first.is_a?(PriceCondition)} #{Condition.find(first.id).class.name}"
    dup = PriceCondition.new(value: "t")
    out << "validates: #{dup.valid?} #{PriceCondition.new(value: "p").valid?}"
    out << "serialize: #{PriceCondition.first.serializable_hash.keys.sort.join(",")}"
    Asset.create!(name: "a", type: "png")
    BigAsset.create!(name: "b", type: "jpg")
    out << "nil: #{Asset.count} #{BigAsset.count} #{Asset.find_by(name: "a").type} #{BigAsset.new.type.inspect} #{Asset.inheritance_column.inspect} #{Asset.order(:id).map { |a| a.class.name }.join(",")} #{Asset.first.serializable_hash.key?("type")}"
    big = BigAsset.find_by(name: "b")
    out << "plain-class: #{BigAsset.first.class.name} #{big.class.name} #{BigAsset.find(big.id).class.name} #{BigAsset.where(name: "a").to_a.first.class.name} #{BigAsset.where(name: "b").select(:id, :name).first.class.name} #{BigAsset.where(name: "b").last.label} #{Asset.find(big.id).class.name} #{BigAsset.all.map(&:label).join(",")}"
    Tile.create!(name: "t", type: "x")
    WideTile.create!(name: "w")
    out << "disabled: #{Tile.count} #{WideTile.count} #{WideTile.new.type.inspect} #{Tile.inheritance_column} #{Tile.order(:id).map { |t| t.class.name }.join(",")} #{WideTile.all.order(:id).map { |t| t.class.name }.join(",")}"
    out << "base-new: #{Condition.new(attribute_name: "TitleCondition", value: "b").class.name} #{Condition.new("attribute_name" => "PriceCondition").class.name} #{Condition.create!(attribute_name: "PriceCondition", value: "pp").class.name} #{Condition.new(value: "x").class.name} #{Condition.new(attribute_name: "").class.name} #{TitleCondition.new(attribute_name: "TitleCondition").class.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 8] = [
    "custom: 2 1 3 TitleCondition data TitleCondition attribute_name attribute_name",
    "hydrate: TitleCondition,PriceCondition,TitleCondition true false TitleCondition",
    "validates: true false",
    "serialize: id,type,value",
    "nil: 2 2 png nil \"\" Asset,Asset true",
    "plain-class: BigAsset BigAsset BigAsset BigAsset BigAsset big b Asset big a,big b",
    "disabled: 2 2 nil _type_disabled Tile,Tile WideTile,WideTile",
    "base-new: TitleCondition PriceCondition PriceCondition Condition Condition TitleCondition",
];

/// What native Rails requires, in order.
const LOADS: [&str; 9] = [
    "app/models/application_record",
    "app/models/condition",
    "app/models/title_condition",
    "app/models/price_condition",
    "app/models/asset",
    "app/models/big_asset",
    "app/models/tile",
    "app/models/wide_tile",
    "app/models/inheritance_column_probe",
];

const FILES: [(&str, &str); 13] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/condition.rb", CONDITION),
    ("app/models/title_condition.rb", TITLE_CONDITION),
    ("app/models/price_condition.rb", PRICE_CONDITION),
    ("app/models/asset.rb", ASSET),
    ("app/models/big_asset.rb", BIG_ASSET),
    ("app/models/tile.rb", TILE),
    ("app/models/wide_tile.rb", WIDE_TILE),
    ("app/models/inheritance_column_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn inheritance_column_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "inheritance-column",
        &FILES,
        &emitted,
        "rh_spec_inheritance_column",
        "InheritanceColumnProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
