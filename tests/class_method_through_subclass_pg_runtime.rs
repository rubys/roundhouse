//! PostgreSQL: class method through subclass, on the Ruby target against native Rails.
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
    kind character varying,
    name character varying,
    CONSTRAINT assets_pkey PRIMARY KEY (id)
);

CREATE TABLE animals (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    sound character varying,
    CONSTRAINT animals_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_class_method_through_subclass
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// A model with its own class methods; the inheritance column is off.
const ASSET: &str = r#"class Asset < ApplicationRecord
  self.inheritance_column = nil

  scope :named, ->(n) { where(name: n) }

  def self.recent
    order(:id)
  end

  def self.newest
    self.order(id: :desc).first
  end

  def self.everything
    all.to_a
  end

  def self.first_two
    first(2)
  end

  def self.named_b
    named("b")
  end

  def self.of_kind(k)
    where(kind: k).order(:id)
  end

  def self.xs
    where(kind: "x").order(:id)
  end

  def self.found(n)
    where(name: n).first
  end

  def self.x_list
    where(kind: "x").to_a
  end

  def self.by_name(n)
    find_by(name: n)
  end

  def self.how_many
    count
  end
end
"#;

const BIG_ASSET: &str = r#"class BigAsset < Asset
  def label
    "big #{name}"
  end
end
"#;

/// An STI base with its own class methods.
const ANIMAL: &str = r#"class Animal < ApplicationRecord
  scope :named, ->(n) { where(name: n) }

  def self.recent
    order(:id)
  end

  def self.first_two
    first(2)
  end

  def self.everything
    all.map { |a| a.class.name }
  end
end
"#;

/// An STI subclass with its own class methods.
const DOG: &str = r#"class Dog < Animal
  def self.loud
    where(sound: "woof").order(:id)
  end

  def self.newest
    self.order(id: :desc).first
  end

  def self.every
    all.map(&:name)
  end

  def self.named_rex
    named("rex")
  end

  def self.first_one
    first(1)
  end
end
"#;

const PUPPY: &str = r#"class Puppy < Dog
end
"#;

const CAT: &str = r#"class Cat < Animal
end
"#;

const PROBE: &str = r##"class ClassMethodThroughSubclassProbe
  def self.lines
    out = []
    Asset.create!(name: "a", kind: "x")
    Asset.create!(name: "b", kind: "y")
    Asset.create!(name: "c", kind: "x")
    out << "model via subclass: #{BigAsset.recent.map(&:label).join(",")} #{BigAsset.newest.label} #{BigAsset.first_two.map(&:label).join(",")}"
    out << "model via subclass scope: #{BigAsset.named_b.map(&:label).join(",")} #{BigAsset.of_kind("x").map(&:label).join(",")} #{BigAsset.recent.where(kind: "y").first.label}"
    out << "model via subclass where: #{BigAsset.xs.map(&:label).join(",")} #{BigAsset.found("b").label}"
    out << "model via subclass folded: #{BigAsset.everything.map { |a| a.class.name }.join(",")} #{BigAsset.x_list.map(&:label).join(",")} #{BigAsset.by_name("c").label} #{BigAsset.how_many}"
    out << "model itself folded: #{Asset.everything.map { |a| a.class.name }.join(",")} #{Asset.x_list.map { |a| a.class.name }.join(",")} #{Asset.by_name("c").class.name} #{Asset.how_many}"
    out << "model itself: #{Asset.recent.map { |a| a.class.name }.join(",")} #{Asset.newest.class.name} #{Asset.first_two.map(&:name).join(",")} #{Asset.of_kind("y").map { |a| a.class.name }.join(",")} #{Asset.found("c").class.name}"
    out
  end
end
"##;

/// A model's class methods reached through a subclass that load all
/// rows, and the STI shapes whose type condition is a runtime one.
const GAP_PROBE: &str = r##"class StiClassMethodProbe
  def self.lines
    out = []
    Dog.create!(name: "rex", sound: "woof")
    Cat.create!(name: "tom", sound: "meow")
    Dog.create!(name: "fido", sound: "growl")
    Puppy.create!(name: "bit", sound: "woof")
    Cat.create!(name: "rex", sound: "woof")
    out << "sti base via subclass: #{Dog.recent.map(&:name).join(",")} #{Cat.first_two.map(&:name).join(",")} #{Dog.everything.join(",")} #{Puppy.recent.map(&:name).join(",")}"
    out << "sti base itself: #{Animal.recent.map { |a| a.class.name }.join(",")} #{Animal.first_two.map(&:name).join(",")}"
    out << "sti own: #{Dog.loud.map(&:name).join(",")} #{Dog.newest.name} #{Dog.every.join(",")} #{Dog.named_rex.map { |a| a.class.name }.join(",")} #{Dog.first_one.map(&:name).join(",")}"
    out << "sti own inherited: #{Puppy.loud.map(&:name).join(",")} #{Puppy.newest.name} #{Puppy.every.join(",")} #{Puppy.named_rex.size}"
    out << "sti unscoped: #{Dog.recent.unscoped.count} #{Puppy.loud.unscoped.count} #{Animal.recent.unscoped.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 6] = [
    "model via subclass: big a,big b,big c big c big a,big b",
    "model via subclass scope: big b big a,big c big b",
    "model via subclass where: big a,big c big b",
    "model via subclass folded: BigAsset,BigAsset,BigAsset big a,big c big c 3",
    "model itself folded: Asset,Asset,Asset Asset,Asset Asset 3",
    "model itself: Asset,Asset,Asset Asset a,b Asset Asset",
];

const GAP_EXPECTED: [&str; 5] = [
    "sti base via subclass: rex,fido,bit tom,rex Dog,Dog,Puppy bit",
    "sti base itself: Dog,Cat,Dog,Puppy,Cat rex,tom",
    "sti own: rex,bit bit rex,fido,bit Dog rex",
    "sti own inherited: bit bit bit 0",
    "sti unscoped: 3 1 5",
];

/// What native Rails requires, in order.
const LOADS: [&str; 9] = [
    "app/models/application_record",
    "app/models/asset",
    "app/models/big_asset",
    "app/models/animal",
    "app/models/dog",
    "app/models/puppy",
    "app/models/cat",
    "app/models/class_method_through_subclass_probe",
    "app/models/sti_class_method_probe",
];

const FILES: [(&str, &str); 13] = [
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
    ("app/models/animal.rb", ANIMAL),
    ("app/models/dog.rb", DOG),
    ("app/models/puppy.rb", PUPPY),
    ("app/models/cat.rb", CAT),
    ("app/models/class_method_through_subclass_probe.rb", PROBE),
    ("app/models/sti_class_method_probe.rb", GAP_PROBE),
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

/// A model's own class methods seed on `self`; an author-written
/// constant would keep its constant.
#[test]
#[ignore = "fork spec: postgresql"]
fn model_class_methods_seed_a_relation_on_self() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let asset = std::fs::read_to_string(emitted.join("app/models/asset.rb")).unwrap();
    for seeded in [
        "ActiveRecord::Relation.new(self).where(kind: k).order(:id)",
        "ActiveRecord::Relation.new(self).where(kind: \"x\").order(:id)",
    ] {
        assert!(asset.contains(seeded), "missing {seeded}:\n{asset}");
    }
    assert!(
        !asset.contains("ActiveRecord::Relation.new(Asset)"),
        "{asset}"
    );
    // The arel-folded `all.to_a` / `where(kind: "x").to_a` hydrate
    // through `self`, and the bare `find_by` stays on `self`.
    assert!(asset.contains("results << from_stmt(stmt)"), "{asset}");
    assert!(!asset.contains("Asset.from_stmt"), "{asset}");
    assert!(!asset.contains("Asset.find_by"), "{asset}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn model_class_methods_through_a_subclass_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "class-method-through-subclass",
        &FILES,
        &emitted,
        "rh_spec_class_method_through_subclass",
        "ClassMethodThroughSubclassProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// An STI subclass's relation carries its type condition at runtime
/// (`_apply_type_condition`), `unscoped` included: a base class method
/// reached through `Dog` reads Dogs, a subclass's own bare scope call
/// (`named("rex")`) runs on it, and `Puppy` inheriting `Dog.loud` reads
/// Puppies.
#[test]
#[ignore = "fork spec: postgresql"]
fn sti_class_methods_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-class-method",
        &FILES,
        &emitted,
        "rh_spec_sti_class_method",
        "StiClassMethodProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, GAP_EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
