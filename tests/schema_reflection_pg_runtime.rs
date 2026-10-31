//! PostgreSQL: schema reflection, on the Ruby target against native Rails.
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
    name character varying(255) DEFAULT ''::character varying NOT NULL,
    note text,
    public boolean DEFAULT true,
    latitude double precision,
    amount numeric(10,2) DEFAULT 0.0 NOT NULL,
    visits integer DEFAULT 0,
    small smallint,
    token uuid DEFAULT gen_random_uuid(),
    created_at timestamp(6) without time zone NOT NULL,
    opened_on date,
    products_count integer,
    legacy character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);

CREATE TABLE products (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);

CREATE TABLE vehicles (
    id bigserial NOT NULL,
    type character varying,
    wheels integer,
    CONSTRAINT vehicles_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_schema_reflection
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  self.ignored_columns = ["legacy"]

  has_many :products
end
"#;

const PRODUCT: &str = r#"class Product < ApplicationRecord
  belongs_to :shop
end
"#;

const VEHICLE: &str = "class Vehicle < ApplicationRecord\nend\n";

const CAR: &str = "class Car < Vehicle\nend\n";

const PROBE: &str = r##"class SchemaReflectionProbe
  def self.column_line(c)
    [c.class.name, c.name, c.type, c.sql_type, c.null, c.default, c.default_function, c.limit, c.precision, c.scale].inspect
  end

  def self.lines
    out = []
    out << "column_names: #{Shop.column_names.inspect}"
    out << "columns_hash keys == column_names: #{Shop.columns_hash.keys == Shop.column_names}"
    out << "frozen: #{[Shop.columns_hash.frozen?, Shop.columns.frozen?, Shop.column_names.frozen?].inspect}"
    Shop.columns.each { |c| out << "column: #{column_line(c)}" }
    id = Shop.columns_hash["id"]
    out << "id: #{[id.serial?, id.auto_incremented_by_db?, id.has_default?, id.virtual?].inspect}"
    visits = Shop.columns_hash["visits"]
    out << "visits: #{[visits.serial?, visits.auto_incremented_by_db?, visits.has_default?].inspect}"
    out << "same column: #{Shop.columns_hash["name"] == Shop.columns[1]}"
    out << "content_columns: #{Shop.content_columns.map(&:name).inspect}"
    missing = Shop.column_for_attribute("nope")
    out << "column_for_attribute: #{[missing.class.name, missing.name, missing.type, missing.null].inspect}"
    out << "product columns: #{Product.columns.map { |c| [c.name, c.type] }.inspect}"
    out << "car columns: #{Car.column_names.inspect}"
    begin
      ApplicationRecord.columns_hash
      out << "abstract columns_hash: answered"
    rescue ActiveRecord::TableNotSpecified => e
      out << "abstract columns_hash: #{e.class.name} #{e.message}"
    end
    out << "base_class: #{[Shop.base_class, Car.base_class, Vehicle.base_class, ApplicationRecord.base_class].map(&:name).inspect}"
    out << "base_class?: #{[Shop.base_class?, Car.base_class?, Vehicle.base_class?].inspect}"
    out << "polymorphic_name: #{[Shop.polymorphic_name, Car.polymorphic_name].inspect}"
    out << "polymorphic_class_for: #{Shop.polymorphic_class_for("Car").name}"
    out << "sanitize_sql_like: #{[Shop.sanitize_sql_like("100%_a\\b"), Shop.sanitize_sql_like("a!_%", "!"), Shop.sanitize_sql_like("plain")].inspect}"
    out << "like query: #{Shop.where("name LIKE ?", "#{Shop.sanitize_sql_like("50%")}%").to_sql.include?("50\\%%")}"
    out << "class has_attribute?: #{[Shop.has_attribute?("id"), Shop.has_attribute?(:name), Shop.has_attribute?("nope"), Shop.has_attribute?("legacy")].inspect}"
    out << "class _has_attribute?: #{[Shop._has_attribute?("id"), Shop._has_attribute?(:id)].inspect}"
    shop = Shop.create!(name: "one", created_at: Time.utc(2026, 1, 2, 3, 4, 5))
    product = shop.products.create!(title: "hat")
    out << "record has_attribute?: #{[shop.has_attribute?("name"), shop.has_attribute?(:name), shop.has_attribute?("nope")].inspect}"
    out << "record _has_attribute?: #{[product._has_attribute?("shop_id"), product._has_attribute?(:shop_id)].inspect}"
    selected = Shop.select(:id).find(shop.id)
    out << "selected has_attribute?: #{[selected.has_attribute?(:id), selected.has_attribute?(:name)].inspect}"
    out << "reflect_on_association: #{Shop.reflect_on_association(:products).macro.inspect}"
    out << "shop associations: #{Shop.reflect_on_all_associations.map { |r| [r.name, r.macro] }.inspect}"
    out << "product associations: #{Product.reflect_on_all_associations.map { |r| [r.name, r.macro] }.inspect}"
    out << "shop belongs_to: #{Shop.reflect_on_all_associations(:belongs_to).map(&:name).inspect}"
    out << "product belongs_to: #{Product.reflect_on_all_associations(:belongs_to).map(&:name).inspect}"
    out << "vehicle associations: #{Vehicle.reflect_on_all_associations.inspect}"
    out << "reflections: #{[Shop.reflections.keys, Product.reflections.keys, Vehicle.reflections.keys, Product.reflections.keys.include?("shop"), Product.reflections["shop"].macro].inspect}"
    out << "normalized_reflections: #{[Shop.normalized_reflections.keys, Product.normalized_reflections.keys].inspect}"
    out << "schema cache: #{[Shop.connection_pool.schema_cache.columns_hash?("shops"), Shop.connection.schema_cache.columns_hash?("vehicles")].inspect}"
    out
  end
end
"##;

const EXPECTED: &[&str] = &[
    r#"column_names: ["id", "name", "note", "public", "latitude", "amount", "visits", "small", "token", "created_at", "opened_on", "products_count"]"#,
    "columns_hash keys == column_names: true",
    "frozen: [true, true, true]",
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "id", :integer, "bigint", false, nil, "nextval('shops_id_seq'::regclass)", 8, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "name", :string, "character varying(255)", false, "", nil, 255, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "note", :text, "text", true, nil, nil, nil, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "public", :boolean, "boolean", true, "true", nil, nil, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "latitude", :float, "double precision", true, nil, nil, nil, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "amount", :decimal, "numeric(10,2)", false, "0.0", nil, nil, 10, 2]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "visits", :integer, "integer", true, "0", nil, 4, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "small", :integer, "smallint", true, nil, nil, 2, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "token", :uuid, "uuid", true, nil, "gen_random_uuid()", nil, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "created_at", :datetime, "timestamp(6) without time zone", false, nil, nil, nil, 6, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "opened_on", :date, "date", true, nil, nil, nil, nil, nil]"#,
    r#"column: ["ActiveRecord::ConnectionAdapters::PostgreSQL::Column", "products_count", :integer, "integer", true, nil, nil, 4, nil, nil]"#,
    "id: [true, true, true, false]",
    "visits: [nil, nil, true]",
    "same column: true",
    r#"content_columns: ["name", "note", "public", "latitude", "amount", "visits", "small", "token", "created_at", "opened_on"]"#,
    r#"column_for_attribute: ["ActiveRecord::ConnectionAdapters::NullColumn", "nope", nil, true]"#,
    r#"product columns: [["id", :integer], ["shop_id", :integer], ["title", :string]]"#,
    r#"car columns: ["id", "type", "wheels"]"#,
    "abstract columns_hash: ActiveRecord::TableNotSpecified ApplicationRecord has no table configured. Set one with ApplicationRecord.table_name=",
    r#"base_class: ["Shop", "Vehicle", "Vehicle", "ApplicationRecord"]"#,
    "base_class?: [true, false, true]",
    r#"polymorphic_name: ["Shop", "Vehicle"]"#,
    "polymorphic_class_for: Car",
    r#"sanitize_sql_like: ["100\\%\\_a\\\\b", "a!!!_!%", "plain"]"#,
    "like query: true",
    "class has_attribute?: [true, true, false, false]",
    "class _has_attribute?: [true, false]",
    "record has_attribute?: [true, true, false]",
    "record _has_attribute?: [true, false]",
    "selected has_attribute?: [true, false]",
    "reflect_on_association: :has_many",
    "shop associations: [[:products, :has_many]]",
    "product associations: [[:shop, :belongs_to]]",
    "shop belongs_to: []",
    "product belongs_to: [:shop]",
    "vehicle associations: []",
    r#"reflections: [["products"], ["shop"], [], true, :belongs_to]"#,
    "normalized_reflections: [[:products], [:shop]]",
    "schema cache: [true, true]",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/product.rb", PRODUCT),
    ("app/models/vehicle.rb", VEHICLE),
    ("app/models/car.rb", CAR),
    ("app/models/schema_reflection_probe.rb", PROBE),
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/shop",
    "app/models/product",
    "app/models/vehicle",
    "app/models/car",
    "app/models/schema_reflection_probe",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn model_schema_reflection_reads_the_postgresql_catalog_as_rails_does() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "schema-reflection",
        &FILES,
        &emitted,
        "rh_spec_schema_reflection",
        "SchemaReflectionProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The emitted runtime carries Rails' column, schema cache and model
/// schema surface, and names what it does not port.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_emitted_runtime_carries_the_schema_reflection_surface() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let adapter = read("runtime/active_record_postgresql_adapter.rb");
    for needle in [
        "class SqlTypeMetadata",
        "class Column\n",
        "class NullColumn < Column",
        "class Column < ConnectionAdapters::Column",
        "def columns(table_name)",
        "def column_definitions(table_name)",
        "def new_column_from_field(",
        "result.type_map = AbstractAdapter.schema_result_type_map",
        "def schema_cache",
    ] {
        assert!(adapter.contains(needle), "adapter lacks {needle:?}");
    }
    let handling = read("runtime/active_record_connection_handling.rb");
    for needle in [
        "class SchemaCache",
        "class SchemaReflection",
        "class BoundSchemaReflection",
        "def columns_hash",
        "def column_for_attribute(",
        "ActiveRecord::TableNotSpecified",
        // CRuby only: it reads the registry through singleton_methods
        // and instance_variable_get.
        "def reflect_on_all_associations(",
    ] {
        assert!(handling.contains(needle), "connection handling lacks {needle:?}");
    }
    let connection = read("runtime/active_record/connection.rb");
    for needle in [
        "def self.base_class",
        "def self.polymorphic_name",
        "def self.sanitize_sql_like(",
        "def _has_attribute?(",
    ] {
        assert!(connection.contains(needle), "connection.rb lacks {needle:?}");
    }
}
