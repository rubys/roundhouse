//! PostgreSQL: has many query constraints, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE fulfillment_orders (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    status character varying,
    CONSTRAINT fulfillment_orders_pkey PRIMARY KEY (id)
);

CREATE TABLE fulfillment_order_line_items (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    fulfillment_order_id bigint,
    quantity integer,
    CONSTRAINT fulfillment_order_line_items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_has_many_query_constraints
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
  include HasManyWithQueryConstraints
end
"#;

const REFLECTION_PATCH: &str = r#"module ReflectionWritableForeignKey
  def writable_foreign_key
    return @writable_foreign_key if defined?(@writable_foreign_key)

    case foreign_key
    when String then [foreign_key.to_sym]
    when Symbol then [foreign_key]
    when Array then foreign_key.map(&:to_sym)
    else []
    end
  end

  attr_writer :writable_foreign_key
end

ActiveSupport.on_load(:active_record) do
  ActiveRecord::Reflection::AssociationReflection.prepend(ReflectionWritableForeignKey)
  ActiveRecord::Reflection::ThroughReflection.prepend(ReflectionWritableForeignKey)
end
"#;

const NATIVE_BOOT: &str = r#"require_relative "lib/patches/active_record/reflection_writable_foreign_key"
"#;

const CONCERN: &str = r##"require "patches/active_record/reflection_writable_foreign_key"

module HasManyWithQueryConstraints
  extend ActiveSupport::Concern

  module ClassMethods
    def has_many_with_query_constraints(name, scope = nil, **options, &block)
      query_constraints = options.delete(:query_constraints)
      foreign_key = options.delete(:foreign_key)
      primary_key = options.delete(:primary_key)

      raise ArgumentError, "query_constraints is required" unless query_constraints
      raise ArgumentError, "foreign_key is required" unless foreign_key
      raise ArgumentError, "primary_key is required" unless primary_key

      query_constraints = Array(query_constraints)
      foreign_key = Array(foreign_key)
      primary_key = Array(primary_key)

      unless (foreign_key - query_constraints).empty?
        raise ArgumentError, "foreign_key #{foreign_key.inspect} must be present in query_constraints"
      end

      has_many(
        name,
        scope,
        foreign_key: query_constraints,
        primary_key: primary_key,
        **options,
        &block
      )

      nullify_columns = foreign_key.map(&:to_s)
      reflection = reflect_on_association(name)
      reflection.writable_foreign_key = foreign_key
      base_class = reflection.association_class

      patched = Class.new(base_class) do
        define_method(:nullified_owner_attributes) do
          attrs = nullify_columns.each_with_object({}) { |col, h| h[col] = nil }
          attrs[reflection.type] = nil if reflection.type.present?
          attrs
        end
      end

      reflection.define_singleton_method(:association_class) { patched }
    end
  end
end
"##;

const FULFILLMENT_ORDER: &str = r#"class FulfillmentOrder < ApplicationRecord
  has_many_with_query_constraints :line_items,
    -> { order(:id) },
    class_name: "FulfillmentOrderLineItem",
    query_constraints: [:shop_id, :fulfillment_order_id],
    primary_key: [:shop_id, :id],
    foreign_key: :fulfillment_order_id,
    inverse_of: :fulfillment_order,
    dependent: :destroy

  def total_quantity
    line_items.sum(&:quantity)
  end
end
"#;

const LINE_ITEM: &str = r#"class FulfillmentOrderLineItem < ApplicationRecord
  belongs_to :fulfillment_order, optional: true
end
"#;

const PROBE: &str = r##"class HasManyQueryConstraintsProbe
  def self.lines
    out = []
    fo = FulfillmentOrder.create!(shop_id: 1, status: "open")
    other = FulfillmentOrder.create!(shop_id: 1, status: "open")
    FulfillmentOrderLineItem.create!(shop_id: 1, fulfillment_order_id: fo.id, quantity: 3)
    FulfillmentOrderLineItem.create!(shop_id: 1, fulfillment_order_id: fo.id, quantity: 5)
    FulfillmentOrderLineItem.create!(shop_id: 1, fulfillment_order_id: other.id, quantity: 7)
    fo = FulfillmentOrder.find(fo.id)
    r = FulfillmentOrder.reflect_on_association(:line_items)
    out << "reflection: #{r.macro} #{r.name} #{r.klass.name} #{r.foreign_key.inspect}"
    out << "writable: #{r.writable_foreign_key.inspect}"
    out << "names: #{FulfillmentOrder.reflect_on_all_associations.map(&:name).sort.inspect}"
    out << "read: #{fo.line_items.map(&:quantity).inspect} #{fo.total_quantity}"
    # An unsaved owner holds an appended line and saves it with itself
    # (Fulfillments::FulfillmentOrder.example_fulfillment_orders).
    item = FulfillmentOrderLineItem.new(shop_id: 1, quantity: 11)
    fresh = FulfillmentOrder.new(shop_id: 1, status: "new")
    fresh.line_items << item
    fresh.save!
    out << "appended: #{item.persisted?} #{item.fulfillment_order_id == fresh.id} #{FulfillmentOrder.find(fresh.id).line_items.map(&:quantity).inspect}"
    out << "inverse: #{fo.line_items.first.fulfillment_order.status}"
    out
  end
end
"##;

const EXPECTED: [&str; 6] = [
    "reflection: has_many line_items FulfillmentOrderLineItem [\"shop_id\", \"fulfillment_order_id\"]",
    "writable: [:fulfillment_order_id]",
    "names: [:line_items]",
    "read: [3, 5] 8",
    "appended: true true [11]",
    "inverse: open",
];

const FILES: [(&str, &str); 12] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("lib/patches/active_record/reflection_writable_foreign_key.rb", REFLECTION_PATCH),
    ("app/models/concerns/has_many_with_query_constraints.rb", CONCERN),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/fulfillment_order.rb", FULFILLMENT_ORDER),
    ("app/models/fulfillment_order_line_item.rb", LINE_ITEM),
    ("app/models/has_many_query_constraints_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn has_many_with_query_constraints_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let mut files = FILES.to_vec();
    files.push(("native_boot.rb", NATIVE_BOOT));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "has-many-query-constraints",
        &files,
        &emitted,
        "rh_spec_has_many_query_constraints",
        "HasManyQueryConstraintsProbe",
        &[
            "native_boot",
            "app/models/concerns/has_many_with_query_constraints",
            "app/models/application_record",
            "app/models/fulfillment_order",
            "app/models/fulfillment_order_line_item",
            "app/models/has_many_query_constraints_probe",
        ],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
