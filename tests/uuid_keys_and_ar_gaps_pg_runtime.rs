//! PostgreSQL: uuid keys and ar gaps, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id uuid NOT NULL,
    name character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
CREATE TABLE widgets (
    id uuid NOT NULL,
    ref character varying NOT NULL,
    shop_id uuid,
    label character varying,
    CONSTRAINT widgets_pkey PRIMARY KEY (ref)
);
CREATE UNIQUE INDEX index_widgets_on_shop_id_and_id ON widgets (shop_id, id);
CREATE INDEX index_widgets_on_label_desc ON widgets (label DESC) WHERE (label IS NOT NULL);
CREATE INDEX index_widgets_on_lower_label ON widgets (lower((label)::text));
CREATE TABLE parts (
    id uuid NOT NULL,
    widget_id uuid NOT NULL,
    kind character varying,
    CONSTRAINT parts_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_uuid_keys_and_ar_gaps
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  def self.uuid_attributes(*names)
    names.each do |name|
      attribute name.to_sym, :uuid_as_integer
    end
  end
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  uuid_attributes :id
  has_many :gadgets, class_name: "Widget"
  has_many :parts, through: :gadgets
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  OID_BASE = 0x51500000000000000000

  self.primary_key = :id
  uuid_attributes :id, :shop_id
  belongs_to :shop, optional: true
  has_many :parts
  before_create :generate_oid

  attribute :flagged, :boolean, default: false

  def generate_oid
    self.id ||= OID_BASE + Widget.count + 1
  end
end
"#;

const PART: &str = r#"class Part < ApplicationRecord
  uuid_attributes :id, :widget_id
  belongs_to :widget
end
"#;

const UUID_AS_INTEGER: &str = r##"module Probe
  class UuidAsInteger < ActiveModel::Type::Value
    ACCEPTABLE_UUID = /\A\h{8}-?\h{4}-?\h{4}-?\h{4}-?\h{12}\z/

    def type
      :uuid_as_integer
    end

    def deserialize(value)
      return if value.nil?
      cast_value(value)
    end

    def cast_value(value)
      if value.is_a?(Integer)
        value
      elsif value.is_a?(String) && ACCEPTABLE_UUID.match?(value)
        value.delete("-").to_i(16)
      elsif value.respond_to?(:to_i)
        value.to_i
      end
    end

    def serialize(value)
      return if value.nil?
      int = cast_value(value)
      return if int.nil?
      hex = int.to_s(16).rjust(32, "0")
      "#{hex[0, 8]}-#{hex[8, 4]}-#{hex[12, 4]}-#{hex[16, 4]}-#{hex[20, 12]}"
    end
  end

  # own uuid type is its uuid.
  module UuidIntegerCast
    def cast_value(value)
      return super unless value.is_a?(Integer)
      hex = value.to_s(16).rjust(32, "0")
      "#{hex[0, 8]}-#{hex[8, 4]}-#{hex[12, 4]}-#{hex[16, 4]}-#{hex[20, 12]}"
    end
  end
end

ActiveSupport.on_load(:active_record_postgresqladapter) do
  ActiveRecord::ConnectionAdapters::PostgreSQL::OID::Uuid.prepend(Probe::UuidIntegerCast)
end
"##;

const INITIALIZER: &str = r#"ActiveRecord::Type.register(:uuid_as_integer, Probe::UuidAsInteger, adapter: :postgresql)
"#;

const PROBE: &str = r##"class UuidKeysProbe
  def self.lines
    out = []
    shop = Shop.create!(id: 0x1234, name: "a")
    w = Widget.create!(ref: "r1", shop: shop, label: "b")
    out << "key: #{Widget.primary_key.inspect} #{w.id.class} #{w.id == Widget::OID_BASE + 1} #{w.ref}"
    out << "find: #{Widget.find(w.id).ref} #{Widget.where(id: w.id).count}"
    w.label = "c"
    out << "update: #{w.save} #{Widget.find(w.id).label}"
    part = Part.create!(id: 0x77, widget_id: w.id, kind: "k")
    out << "joins where joined model: #{Shop.joins(gadgets: :parts).where(parts: { widget_id: w.id }).count}"
    out << "joins where through table: #{Shop.joins(:parts).where(widgets: { shop_id: shop.id }).count}"
    out << "joins where through table id: #{Shop.joins(:parts).where(widgets: { id: w.id }).pluck(:name).inspect}"
    out << "attribute_present?: #{w.attribute_present?("label")} #{w.attribute_present?(:ref)} #{w.attribute_present?("shop_id")}"
    w.label = ""
    out << "attribute_present? blank: #{w.attribute_present?("label")}"
    w.label = nil
    out << "attribute_present? nil: #{w.attribute_present?("label")}"
    out << "predicate: #{w.flagged?.inspect}"
    w.flagged = "1"
    out << "predicate set: #{w.flagged?.inspect} #{w.flagged.inspect}"
    cache = Widget.connection.schema_cache
    cache.indexes("widgets").each do |index|
      out << "index: #{index.name} #{index.unique.inspect} #{index.columns.inspect} #{index.where.inspect} #{index.orders.inspect} #{index.using.inspect} #{index.table}"
    end
    out << "indexes cached: #{cache.indexes("widgets").equal?(cache.indexes("widgets"))}"
    out << "indexes none: #{cache.indexes("parts").map(&:name).inspect} #{cache.indexes("no_such_table").inspect}"
    out << "connection indexes: #{Widget.connection.indexes("widgets").map(&:name).inspect}"
    out
  end
end
"##;

/// What native Rails requires, in order.
const LOADS: [&str; 7] = [
    "lib/probe/uuid_as_integer",
    "config/initializers/uuid_types",
    "app/models/application_record",
    "app/models/shop",
    "app/models/widget",
    "app/models/part",
    "app/models/uuid_keys_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/widget.rb", WIDGET),
    ("app/models/part.rb", PART),
    ("app/models/uuid_keys_probe.rb", PROBE),
    ("lib/probe/uuid_as_integer.rb", UUID_AS_INTEGER),
];

const INITIALIZER_FILE: (&str, &str) = ("config/initializers/uuid_types.rb", INITIALIZER);

fn files() -> Vec<(&'static str, &'static str)> {
    let mut all = FILES.to_vec();
    all.push(INITIALIZER_FILE);
    all
}

fn overlay() -> emit_and_run::Overlay {
    files().iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the declared key is what the INSERT answers, and the
/// typed boolean has its predicate.
#[test]
#[ignore = "fork spec: postgresql"]
fn declared_key_over_another_primary_key_column_is_the_insert_answer() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let model = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
    let insert = model.split("def _adapter_insert").nth(1).expect("_adapter_insert");
    let insert = insert.split("\n  end\n").next().unwrap();
    assert!(insert.trim_end().ends_with("@id"), "the INSERT answers the declared key:\n{insert}");
    assert!(!insert.trim_end().ends_with("@ref"), "the INSERT answers the table's key:\n{insert}");
    assert!(model.contains("def flagged?\n"), "missing the predicate in:\n{model}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn uuid_keys_and_ar_gaps_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "uuid-keys-and-ar-gaps",
        &files(),
        &emitted,
        "rh_spec_uuid_keys_and_ar_gaps",
        "UuidKeysProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}

const EXPECTED: [&str; 17] = [
    "key: \"id\" Integer true r1",
    "find: r1 1",
    "update: true c",
    "joins where joined model: 1",
    "joins where through table: 1",
    "joins where through table id: [\"a\"]",
    "attribute_present?: true true true",
    "attribute_present? blank: false",
    "attribute_present? nil: false",
    "predicate: false",
    "predicate set: true true",
    "index: index_widgets_on_label_desc false [\"label\"] \"(label IS NOT NULL)\" :desc :btree widgets",
    "index: index_widgets_on_lower_label false \"lower((label)::text)\" nil {} :btree widgets",
    "index: index_widgets_on_shop_id_and_id true [\"shop_id\", \"id\"] nil {} :btree widgets",
    "indexes cached: true",
    "indexes none: [] []",
    "connection indexes: [\"index_widgets_on_label_desc\", \"index_widgets_on_lower_label\", \"index_widgets_on_shop_id_and_id\"]",
];
