//! PostgreSQL: ar types 2, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE products (
    id bigserial NOT NULL,
    title character varying,
    publish_on time without time zone,
    meta json,
    data jsonb,
    created_at timestamp(6) without time zone,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);

CREATE TABLE connections (
    id bigserial NOT NULL,
    object_id bigint,
    object_type character varying,
    CONSTRAINT connections_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_ar_types_2
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const PRODUCT: &str = "class Product < ApplicationRecord\nend\n";

const CONNECTION: &str = r#"class Connection < ApplicationRecord
  belongs_to :connectable, polymorphic: true, foreign_key: "object_id", foreign_type: "object_type"
end
"#;

const CONNECTIONS_YML: &str = r#"one:
  connectable: first (Product)
"#;

const PRODUCTS_YML: &str = "first:
  title: fixture
";

const PROBE: &str = r##"class ArTypesProbe
  def self.lines
    out = []
    %w[publish_on meta data].each do |name|
      type = Product.type_for_attribute(name)
      out << "type #{name}: #{type.type.inspect} #{type.precision.inspect}"
    end
    time = Product.type_for_attribute("publish_on")
    cast = time.cast("10:30:15.25")
    out << "time cast: #{cast.class} #{[cast.year, cast.month, cast.day, cast.hour, cast.min, cast.sec, cast.usec].inspect}"
    out << "time cast nil: #{time.cast("").inspect} #{time.cast("nope").inspect}"
    out << "time serialize: #{time.serialize(cast).is_a?(Delegator)} #{time.serialize(cast).hour}"
    json = Product.type_for_attribute("meta")
    out << "json: #{json.deserialize('{"a":[1,2]}').inspect} #{json.serialize({ "a" => [1, 2] }).inspect} #{json.serialize(nil).inspect}"
    jsonb = Product.type_for_attribute("data")
    out << "jsonb: #{jsonb.class.superclass} #{jsonb.deserialize('{"b":true}').inspect} #{jsonb.changed_in_place?('{"b":true}', { "b" => false })}"
    product = Product.create!(title: "first", created_at: Time.utc(2026, 1, 2, 3, 4, 5, 678901))
    dumped = product.attributes_for_database
    out << "for database: #{dumped.keys.sort.inspect} #{dumped["id"] == product.id} #{dumped["title"].inspect} #{dumped["created_at"].utc.usec}"
    set = Product.attributes_builder.build_from_database(dumped, {})
    loaded = Product.allocate.init_with_attributes(set)
    out << "round trip: #{loaded.class} #{loaded.id == product.id} #{loaded.title.inspect} #{loaded.created_at.utc.usec} #{loaded.persisted?} #{loaded.new_record?}"
    fresh = Product.allocate.init_with_attributes(Product.attributes_builder.build_from_database({ "title" => "new" }, {}), true)
    out << "round trip new: #{fresh.title.inspect} #{fresh.new_record?}"
    connection = Connection.create!(connectable: product)
    out << "foreign_type write: #{connection.object_id == product.id} #{connection.object_type.inspect}"
    reread = Connection.find(connection.id)
    out << "foreign_type read: #{reread.connectable.class} #{reread.connectable.id == product.id}"
    out << "reflection: #{Connection.reflect_on_association(:connectable).foreign_type.inspect} #{Connection.reflect_on_association(:connectable).foreign_key.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 14] = [
    "type publish_on: :time 6",
    "type meta: :json nil",
    "type data: :jsonb nil",
    "time cast: Time [2000, 1, 1, 10, 30, 15, 250000]",
    "time cast nil: nil nil",
    "time serialize: true 10",
    r#"json: {"a" => [1, 2]} "{\"a\":[1,2]}" nil"#,
    r#"jsonb: ActiveRecord::Type::Json {"b" => true} true"#,
    r#"for database: ["created_at", "data", "id", "meta", "publish_on", "title"] true "first" 678901"#,
    r#"round trip: Product true "first" 678901 true false"#,
    r#"round trip new: "new" true"#,
    r#"foreign_type write: true "Product""#,
    "foreign_type read: Product true",
    r#"reflection: "object_type" "object_id""#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 4] =
    ["app/models/application_record", "app/models/product", "app/models/connection", "app/models/ar_types_probe"];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/product.rb", PRODUCT),
    ("app/models/connection.rb", CONNECTION),
    ("app/models/ar_types_probe.rb", PROBE),
    ("test/fixtures/connections.yml", CONNECTIONS_YML),
    ("test/fixtures/products.yml", PRODUCTS_YML),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the schema specs carry the PostgreSQL time
/// precision and the json/jsonb kinds, and the polymorphic fixture label
/// writes the declared `foreign_type:` column.
#[test]
#[ignore = "fork spec: postgresql"]
fn time_json_specs_and_foreign_type_fixture_compile() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let product = std::fs::read_to_string(emitted.join("app/models/product.rb")).unwrap();
    for expected in [r#""publish_on" => [:time, 6]"#, r#""meta" => [:json, nil]"#, r#""data" => [:jsonb, nil]"#] {
        assert!(product.contains(expected), "missing {expected:?} in:\n{product}");
    }
    let fixtures = std::fs::read_to_string(emitted.join("test/fixtures/connections.rb")).unwrap();
    assert!(fixtures.contains("object_type"), "fixtures:\n{fixtures}");
    assert!(!fixtures.contains("connectable_type"), "fixtures:\n{fixtures}");
}

/// The models natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn ar_types_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("ar-types-2", &FILES, &emitted, "rh_spec_ar_types_2", "ArTypesProbe", &LOADS) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
