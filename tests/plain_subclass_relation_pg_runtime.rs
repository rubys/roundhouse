//! PostgreSQL: plain subclass relation, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE assets (
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
  database: rh_spec_plain_subclass_relation
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ASSET: &str = r#"class Asset < ApplicationRecord
  self.inheritance_column = nil

  scope :named, ->(n) { where(name: n) }
end
"#;

/// Its own class methods root bare and `self.` chains on itself.
const BIG_ASSET: &str = r#"class BigAsset < Asset
  def self.recent
    order(:id)
  end

  def self.newest
    self.order(id: :desc).first
  end

  def self.of_type(t)
    where(type: t).order(:id)
  end

  def self.labels
    all.map(&:label)
  end

  def self.first_two
    first(2)
  end

  def self.named_b
    named("b")
  end

  def self.selfish
    self.named("c").order(:id)
  end

  def self.picked
    select(:id, :name).limit(2).order(:id)
  end

  def label
    "big #{name}"
  end
end
"#;

/// Two levels below the model.
const HUGE_ASSET: &str = r#"class HugeAsset < BigAsset
end
"#;

const TILE: &str = r#"class Tile < ApplicationRecord
  self.inheritance_column = :_type_disabled
end
"#;

const WIDE_TILE: &str = r#"class WideTile < Tile
end
"#;

const PROBE: &str = r##"class PlainSubclassProbe
  def self.lines
    out = []
    Asset.create!(name: "a", type: "png")
    BigAsset.create!(name: "b", type: "jpg")
    BigAsset.create!(name: "c", type: "gif")
    out << "select: #{BigAsset.select(:id, :name).order(:id).map { |a| [a.class.name, a.name] }.inspect}"
    out << "order: #{BigAsset.order(:id).map(&:label).join(",")} #{BigAsset.order(id: :desc).first.label} #{BigAsset.order(:id).last.class.name}"
    out << "chain: #{BigAsset.order(:name).where(type: "gif").map(&:label).join(",")} #{BigAsset.select(:id).order(:id).count} #{BigAsset.order(:id).named("b").map(&:label).join(",")}"
    out << "scope: #{BigAsset.named("b").map(&:label).join(",")} #{BigAsset.named("c").order(:id).first.class.name}"
    out << "limit: #{BigAsset.limit(2).order(:id).map(&:label).join(",")} #{BigAsset.offset(1).order(:id).limit(1).first.label}"
    out << "terminals: #{BigAsset.pluck(:name).sort.join(",")} #{BigAsset.ids.size} #{BigAsset.first(2).map(&:label).join(",")} #{BigAsset.last(1).map(&:class).map(&:name).join(",")}"
    out << "own: #{BigAsset.recent.map(&:label).join(",")} #{BigAsset.newest.label} #{BigAsset.of_type("jpg").map(&:label).join(",")} #{BigAsset.labels.sort.join(",")} #{BigAsset.first_two.map(&:label).join(",")}"
    out << "own scope: #{BigAsset.named_b.map(&:label).join(",")} #{BigAsset.selfish.first.class.name} #{BigAsset.picked.map { |a| [a.class.name, a.label] }.inspect} #{BigAsset.recent.where(name: "c").first.label}"
    out << "inherited own: #{HugeAsset.recent.map { |a| a.class.name }.join(",")} #{HugeAsset.newest.class.name}"
    out << "deeper: #{HugeAsset.order(:id).map { |a| a.class.name }.join(",")} #{HugeAsset.select(:name).order(:name).first.label}"
    out << "parent: #{Asset.order(:id).map { |a| a.class.name }.join(",")} #{Asset.select(:id).order(:id).first.class.name}"
    Tile.create!(name: "t", type: "x")
    WideTile.create!(name: "w")
    out << "tile: #{WideTile.order(:id).map { |t| t.class.name }.join(",")} #{WideTile.select(:id, :name).order(name: :desc).first.name} #{WideTile.where(name: "w").order(:id).first.class.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 12] = [
    r#"select: [["BigAsset", "a"], ["BigAsset", "b"], ["BigAsset", "c"]]"#,
    r#"order: big a,big b,big c big c BigAsset"#,
    r#"chain: big c 3 big b"#,
    r#"scope: big b BigAsset"#,
    r#"limit: big a,big b big b"#,
    r#"terminals: a,b,c 3 big a,big b BigAsset"#,
    r#"own: big a,big b,big c big c big b big a,big b,big c big a,big b"#,
    r#"own scope: big b BigAsset [["BigAsset", "big a"], ["BigAsset", "big b"]] big c"#,
    r#"inherited own: HugeAsset,HugeAsset,HugeAsset HugeAsset"#,
    r#"deeper: HugeAsset,HugeAsset,HugeAsset big a"#,
    r#"parent: Asset,Asset,Asset Asset"#,
    r#"tile: WideTile,WideTile w WideTile"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 7] = [
    "app/models/application_record",
    "app/models/asset",
    "app/models/big_asset",
    "app/models/huge_asset",
    "app/models/tile",
    "app/models/wide_tile",
    "app/models/plain_subclass_probe",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\nend\n",
    ),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/asset.rb", ASSET),
    ("app/models/big_asset.rb", BIG_ASSET),
    ("app/models/huge_asset.rb", HUGE_ASSET),
    ("app/models/tile.rb", TILE),
    ("app/models/wide_tile.rb", WIDE_TILE),
    ("app/models/plain_subclass_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| {
            app.write(path, text)
        })
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

/// The emitted relation is seeded on the subclass, not left on Base.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_plain_subclass_chain_seeds_a_relation_on_the_subclass() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let probe =
        std::fs::read_to_string(emitted.join("app/models/plain_subclass_probe.rb")).unwrap();
    for seeded in [
        "ActiveRecord::Relation.new(BigAsset).select(:id, :name)",
        "ActiveRecord::Relation.new(BigAsset).order(:id)",
        "ActiveRecord::Relation.new(HugeAsset).order(:id)",
        "ActiveRecord::Relation.new(WideTile).order(:id)",
        "ActiveRecord::Relation.new(Asset).order(:id)",
    ] {
        assert!(probe.contains(seeded), "missing {seeded}:\n{probe}");
    }
    // The inherited scope stays a class-method call on the subclass.
    assert!(probe.contains("BigAsset.named(\"b\")"), "{probe}");
    // The subclass's own class methods seed on `self`, the receiving
    // class (HugeAsset when it inherits them).
    let big = std::fs::read_to_string(emitted.join("app/models/big_asset.rb")).unwrap();
    for seeded in [
        "ActiveRecord::Relation.new(self).order(:id)",
        "ActiveRecord::Relation.new(self).order(id: :desc).first",
        "ActiveRecord::Relation.new(self).where(type: t).order(:id)",
        "ActiveRecord::Relation.new(self).map { |x| x.label }",
        "ActiveRecord::Relation.new(self).first_n(2)",
        "ActiveRecord::Relation.new(self).select(:id, :name).limit(2).order(:id)",
    ] {
        assert!(big.contains(seeded), "missing {seeded}:\n{big}");
    }
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn plain_subclass_relations_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "plain-subclass-relation",
        &FILES,
        &emitted,
        "rh_spec_plain_subclass_relation",
        "PlainSubclassProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
