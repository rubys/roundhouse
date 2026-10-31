//! PostgreSQL: record serialization, on the Ruby target against native Rails.
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
    plan character varying,
    created_at timestamp(6) without time zone,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
CREATE TABLE products (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    price numeric(10,2),
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
CREATE TABLE vehicles (
    id bigserial NOT NULL,
    type character varying,
    wheels integer,
    CONSTRAINT vehicles_pkey PRIMARY KEY (id)
);
"#;

/// The same tables for the SQLite tree.
const SCHEMA: &str = r#"ActiveRecord::Schema[8.0].define(version: 1) do
  create_table :shops do |t|
    t.string :name
    t.string :plan
    t.datetime :created_at
  end
  create_table :products do |t|
    t.bigint :shop_id
    t.string :title
    t.decimal :price, precision: 10, scale: 2
  end
  create_table :vehicles do |t|
    t.string :type
    t.integer :wheels
  end
end
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_record_serialization
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :products

  def label
    "shop #{name}"
  end

  def paid?
    plan == "paid"
  end

  def settings_json
    as_json(only: [:id], methods: [:label, :paid?])
  end

  def without_plan
    as_json(except: [:plan, :created_at])
  end
end
"#;

const PRODUCT: &str = "class Product < ApplicationRecord\n  belongs_to :shop\nend\n";
const VEHICLE: &str = "class Vehicle < ApplicationRecord\nend\n";
const CAR: &str = "class Car < Vehicle\nend\n";

const PROBE: &str = r##"class RecordSerializationProbe
  def self.lines
    out = []
    s = Shop.new(name: "a", plan: "paid")
    out << "new key: #{s.to_key.inspect} #{s.to_param.inspect} #{s.persisted?} #{s.new_record?} #{s.to_model.equal?(s)}"
    out << "new json: #{s.as_json.inspect}"
    s.save!
    out << "saved key: #{s.to_key.inspect} #{s.to_param.inspect} #{s.persisted?} #{s.new_record?}"
    s.products.create!(title: "t", price: 1.5)
    out << "only+methods: #{s.settings_json.inspect}"
    out << "except: #{s.without_plan.inspect}"
    out << "only: #{s.as_json(only: [:name]).inspect}"
    out << "methods: #{s.as_json(only: :name, methods: :label).inspect}"
    out << "root: #{s.as_json(only: :name, root: true).inspect} #{s.as_json(only: :name, root: "x").inspect}"
    out << "sh: #{s.serializable_hash.keys.inspect} #{s.serializable_hash["created_at"].class}"
    out << "sh only: #{s.serializable_hash(only: [:name, :plan]).inspect}"
    out << "sh except: #{s.serializable_hash(except: [:created_at]).keys.inspect}"
    out << "include: #{s.as_json(only: [:name], include: { products: { only: [:title, :price] } }).inspect}"
    out << "include sym: #{s.as_json(only: [:name], include: :products)["products"].map(&:keys).inspect}"
    out << "include arr: #{s.as_json(only: [:name], include: [{ products: { only: :title } }])["products"].inspect}"
    p = Product.first
    out << "include one: #{p.as_json(only: [:title], include: { shop: { only: :name } }).inspect}"
    out << "full keys: #{s.as_json.keys.inspect}"
    t = s.as_json(only: :created_at)["created_at"]
    out << "time: #{t.class} #{t[/\.(\d+)/, 1].size} #{Time.iso8601(t).to_i == s.created_at.to_i}"
    out << "names: #{s.attribute_names.inspect} #{Shop.include_root_in_json.inspect} #{Shop.param_delimiter.inspect}"
    c = Car.create!(wheels: 4)
    out << "sti: #{c.as_json.inspect} #{c.serializable_hash(except: :wheels).inspect} #{c.as_json(only: [:type, :wheels]).inspect}"
    d = Shop.find(s.id)
    d.destroy
    out << "destroyed key: #{d.to_key.inspect} #{d.persisted?} #{d.new_record?} #{d.destroyed?}"
    out
  end
end
"##;

const EXPECTED: [&str; 20] = [
    "new key: nil nil false true true",
    r#"new json: {"id" => nil, "name" => "a", "plan" => "paid", "created_at" => nil}"#,
    r#"saved key: [1] "1" true false"#,
    r#"only+methods: {"id" => 1, "label" => "shop a", "paid?" => true}"#,
    r#"except: {"id" => 1, "name" => "a"}"#,
    r#"only: {"name" => "a"}"#,
    r#"methods: {"name" => "a", "label" => "shop a"}"#,
    r#"root: {"shop" => {"name" => "a"}} {"x" => {"name" => "a"}}"#,
    r#"sh: ["id", "name", "plan", "created_at"] Time"#,
    r#"sh only: {"name" => "a", "plan" => "paid"}"#,
    r#"sh except: ["id", "name", "plan"]"#,
    r#"include: {"name" => "a", "products" => [{"title" => "t", "price" => "1.5"}]}"#,
    r#"include sym: [["id", "shop_id", "title", "price"]]"#,
    r#"include arr: [{"title" => "t"}]"#,
    r#"include one: {"title" => "t", "shop" => {"name" => "a"}}"#,
    r#"full keys: ["id", "name", "plan", "created_at"]"#,
    "time: String 3 true",
    r#"names: ["id", "name", "plan", "created_at"] false "_""#,
    r#"sti: {"id" => 1, "wheels" => 4} {"id" => 1} {"type" => "Car", "wheels" => 4}"#,
    "destroyed key: [1] false false true",
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/shop",
    "app/models/product",
    "app/models/vehicle",
    "app/models/car",
    "app/models/record_serialization_probe",
];

const MODELS: [(&str, &str); 6] = [
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/product.rb", PRODUCT),
    ("app/models/vehicle.rb", VEHICLE),
    ("app/models/car.rb", CAR),
    ("app/models/record_serialization_probe.rb", PROBE),
];

fn pg_files() -> Vec<(&'static str, &'static str)> {
    let mut files = vec![
        ("roundhouse.yml", ""),
        ("config/database.yml", DATABASE_YML),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", STRUCTURE),
    ];
    files.extend(MODELS);
    files
}

fn overlay(files: &[(&str, &str)]) -> emit_and_run::Overlay {
    files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn record_serialization_matches_native_rails_on_postgresql() {
    let files = pg_files();
    let (emitted, errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "record-serialization",
        &files,
        &emitted,
        "rh_spec_record_serialization",
        "RecordSerializationProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The SQLite CRuby tree's Base has no ActiveModel::Serialization: a
/// record's options beyond `only:` and its `serializable_hash` are
/// refused where they are sent, and `only:` and no options are not.
#[test]
#[ignore = "fork spec: postgresql"]
fn record_serialization_beyond_only_is_refused_off_the_postgresql_tree() {
    let mut files = vec![("config/routes.rb", "Rails.application.routes.draw do\nend\n"), ("db/schema.rb", SCHEMA)];
    files.extend(MODELS);
    let (_emitted, errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    // A diagnostic names its send by span; find the send's in its file.
    let refusals = |source: &str, send: &str| {
        let start = source.find(send).unwrap_or_else(|| panic!("{send} not in source"));
        let span = format!("start: {start}, end: {}", start + send.len());
        errors.iter().filter(|e| e.contains(&span) && e.contains("answers ActiveModel::Serializers::JSON")).count()
    };
    for (source, send) in [
        (SHOP, "as_json(only: [:id], methods: [:label, :paid?])"),
        (SHOP, "as_json(except: [:plan, :created_at])"),
        (PROBE, "s.as_json(only: :name, methods: :label)"),
        (PROBE, "s.as_json(only: :name, root: true)"),
        (PROBE, "s.serializable_hash(only: [:name, :plan])"),
        (PROBE, "s.serializable_hash(except: [:created_at])"),
        (PROBE, "s.as_json(only: [:name], include: :products)"),
        (PROBE, "p.as_json(only: [:title], include: { shop: { only: :name } })"),
        (PROBE, "c.serializable_hash(except: :wheels)"),
    ] {
        assert_eq!(refusals(source, send), 1, "{send} not refused:\n{}", errors.join("\n"));
    }
    // `as_json` with no options or an `only:` literal is every tree's.
    for send in ["s.as_json.inspect", "s.as_json(only: [:name])", "s.as_json(only: :created_at)", "c.as_json(only: [:type, :wheels])"] {
        let send = send.trim_end_matches(".inspect");
        let start = PROBE.find(send).unwrap();
        assert!(
            !errors.iter().any(|e| e.contains(&format!("start: {start}, ")) && e.contains("ActiveModel::Serializers::JSON")),
            "{send} refused:\n{}",
            errors.join("\n")
        );
    }
    // No other send is refused: 2 in Shop, 12 in the probe (`root:`
    // twice, the no-option `serializable_hash` twice, `include:` thrice).
    let all = errors.iter().filter(|e| e.contains("answers ActiveModel::Serializers::JSON")).count();
    assert_eq!(all, 14, "{}", errors.join("\n"));
}
