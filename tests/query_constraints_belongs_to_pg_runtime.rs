//! PostgreSQL: query constraints belongs to, on the Ruby target against native Rails.
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

CREATE TABLE people (
    id bigserial NOT NULL,
    shop_id bigint,
    name character varying,
    last_purchase_id bigint,
    CONSTRAINT people_pkey PRIMARY KEY (id)
);

CREATE TABLE purchases (
    id bigserial NOT NULL,
    shop_id bigint,
    total integer,
    CONSTRAINT purchases_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_query_constraints_belongs_to
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
  self.belongs_to_required_by_default = true
  include BelongsToWithQueryConstraints
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

module BelongsToWithQueryConstraints
  extend ActiveSupport::Concern

  module ClassMethods
    def belongs_to_with_query_constraints(name, scope = nil, query_constraints:, foreign_key:, primary_key:, **options)
      query_constraints = Array(query_constraints)
      foreign_key = Array(foreign_key)
      primary_key = Array(primary_key)

      unless (foreign_key - query_constraints).empty?
        raise ArgumentError, "foreign_key #{foreign_key.inspect} must be present in query_constraints"
      end

      required = if options.key?(:optional)
        !options.delete(:optional)
      elsif options.key?(:required)
        options.delete(:required)
      else
        belongs_to_required_by_default
      end

      belongs_to(
        name,
        scope,
        foreign_key: query_constraints,
        primary_key: primary_key,
        optional: true,
        **options,
      )

      reflection = reflect_on_association(name)
      reflection.options[:optional] = !required
      reflection.writable_foreign_key = foreign_key

      # The association class Rails builds for the reflection keeps only the
      # writable key column(s) in its staleness state.
      patched_association_class = Class.new(reflection.association_class) do
        define_method(:stale_state) do
          association = self #: as ActiveRecord::Associations::BelongsToAssociation
          vals = foreign_key.map { |col| association.owner.read_attribute(col) }
          return nil if vals.compact.empty?

          vals
        end
      end
      reflection.define_singleton_method(:association_class) { patched_association_class }

      add_presence_validation(name, foreign_key) if required
    end

    private

    def add_presence_validation(name, fk_columns)
      reflection = reflect_on_association(name)
      condition = lambda { |record|
        record_missing = fk_columns.any? { |col| record.read_attribute(col).nil? || record.attribute_changed?(col) }
        record_missing || (reflection.polymorphic? && record.read_attribute(reflection.foreign_type).nil?)
      }
      validates(name, presence: { message: :required }, if: condition)
    end
  end
end
"##;

const SHOP: &str = r#"class Shop < ApplicationRecord
end
"#;

const PURCHASE: &str = r#"class Purchase < ApplicationRecord
end
"#;

const PERSON: &str = r#"class Person < ApplicationRecord
  belongs_to_with_query_constraints :last_purchase, class_name: "Purchase", inverse_of: false,
    query_constraints: [:shop_id, :last_purchase_id],
    primary_key: [:shop_id, :id],
    foreign_key: :last_purchase_id
  belongs_to_with_query_constraints :home_shop, class_name: "Shop", inverse_of: false, optional: true,
    query_constraints: [:shop_id],
    primary_key: [:id],
    foreign_key: :shop_id

  delegate :total, prefix: "last_purchase", to: :last_purchase, allow_nil: true
end
"#;

const PROBE: &str = r##"class QueryConstraintsBelongsToProbe
  def self.lines
    out = []
    s1 = Shop.create!(name: "one")
    p1 = Purchase.create!(shop_id: s1.id, total: 5)
    Purchase.create!(shop_id: s1.id + 1, total: 9)
    person = Person.create!(name: "ann", shop_id: s1.id, last_purchase_id: p1.id)
    person = Person.find(person.id)
    r = Person.reflect_on_association(:last_purchase)
    out << "reflection: #{r.class.name.split("::").last} #{r.macro} #{r.name} #{r.klass.name} #{r.foreign_key.inspect} #{r.options[:optional].inspect}"
    out << "writable: #{r.writable_foreign_key.inspect}"
    h = Person.reflect_on_association(:home_shop)
    out << "second: #{h.macro} #{h.klass.name} #{h.foreign_key.inspect} #{h.options[:optional].inspect}"
    out << "missing: #{Person.reflect_on_association(:nothing).inspect}"
    out << "names: #{Person.reflect_on_all_associations.map(&:name).sort.inspect}"
    out << "read: #{person.last_purchase.total} #{person.last_purchase_total}"
    out << "primary key: #{r.association_primary_key.inspect} #{h.association_primary_key.inspect}"
    blank = Person.new(name: "bob", shop_id: s1.id)
    out << "required: #{blank.valid?} #{blank.errors.full_messages.inspect}"
    blank.last_purchase_id = p1.id
    out << "present: #{blank.valid?}"
    out
  end
end
"##;

const EXPECTED: [&str; 9] = [
    "reflection: BelongsToReflection belongs_to last_purchase Purchase [\"shop_id\", \"last_purchase_id\"] false",
    "writable: [:last_purchase_id]",
    "second: belongs_to Shop [\"shop_id\"] true",
    "missing: nil",
    "names: [:home_shop, :last_purchase]",
    "read: 5 5",
    "primary key: [\"shop_id\", \"id\"] [\"id\"]",
    "required: false [\"Last purchase must exist\"]",
    "present: true",
];

const FILES: [(&str, &str); 13] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("lib/patches/active_record/reflection_writable_foreign_key.rb", REFLECTION_PATCH),
    ("app/models/concerns/belongs_to_with_query_constraints.rb", CONCERN),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/purchase.rb", PURCHASE),
    ("app/models/person.rb", PERSON),
    ("app/models/query_constraints_belongs_to_probe.rb", PROBE),
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
fn query_constraints_belongs_to_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let mut files = FILES.to_vec();
    files.push(("native_boot.rb", NATIVE_BOOT));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "query-constraints-belongs-to",
        &files,
        &emitted,
        "rh_spec_query_constraints_belongs_to",
        "QueryConstraintsBelongsToProbe",
        &[
            "native_boot",
            "app/models/concerns/belongs_to_with_query_constraints",
            "app/models/application_record",
            "app/models/shop",
            "app/models/purchase",
            "app/models/person",
            "app/models/query_constraints_belongs_to_probe",
        ],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
