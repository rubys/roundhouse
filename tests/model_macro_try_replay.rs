//! PostgreSQL: model macro try replay, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const STRUCTURE: &str = r#"CREATE TABLE products (
    id bigserial NOT NULL,
    price numeric(10,2),
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_try_replay\n";

const CONCERN: &str = r##"module Priced
  extend ActiveSupport::Concern

  class_methods do
    def configure_priced(attributes: [], wrapper:)
      raise(ArgumentError, "attributes must have at least one attribute") if attributes.blank?

      original = try(:priced_options) || {}
      added = attributes.to_h { |attr| [attr.to_s, { wrapper: wrapper }] }
      options = original.merge(added).freeze
      singleton_class.silence_redefinition_of_method(:priced_options)
      define_singleton_method(:priced_options) { options }
    end
  end
end
"##;

const MODEL: &str = r##"class Product < ApplicationRecord
  include Priced

  configure_priced(attributes: [:price], wrapper: proc { |value| "price:#{value}" })
end
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("Gemfile", "source \"https://rubygems.org\"\n")
        .write("roundhouse.yml", "")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write("config/database.yml", DATABASE_YML)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/concerns/priced.rb", CONCERN)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/product.rb", MODEL)
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_macro_reading_its_options_through_try_is_replayed_on_postgresql() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let product = std::fs::read_to_string(dir.join("app/models/product.rb")).expect("product.rb");
    assert!(product.contains("configure_priced(attributes: [:price]"), "the macro call was dropped:\n{product}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_tree_without_object_try_still_refuses_the_macro_by_name() {
    let (_dir, errors) = app()
        .write("roundhouse.yml", "database_adapter: sqlite3\n")
        .write("config/database.yml", "test:\n  adapter: sqlite3\n  database: \":memory:\"\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table \"products\" do |t|\n    t.decimal \"price\"\n  end\nend\n")
        .emit(BuildTarget::Ruby);
    let expected = "model macro call on `Product` not lowered; `configure_priced` calls `try`, which the lowered `Product` does not define";
    assert!(errors.iter().any(|e| e.contains(expected)), "{errors:#?}");
}

const MONEY_BAGS_CONCERN: &str = r##"module Bagged
  extend ActiveSupport::Concern

  class_methods do
    def bags(**kwargs)
      unsafe_self = self #: as untyped
      unsafe_self.configure_bags(**kwargs)

      unsafe_self.bag_options.keys.each do |attribute|
        if respond_to?(:attribute)
          unsafe_self.attribute(attribute, :decimal)
        end

        silence_redefinition_of_method(attribute)
        define_method(attribute) do
          "bag:#{self[attribute]}"
        end
      end
    end

    def configure_bags(attributes: [], packer:)
      raise(ArgumentError, "attributes must have at least one attribute") if attributes.blank?

      original_options = try(:bag_options) || {}
      new_options = attributes.to_h { |attr| [attr.to_s, { packer: packer }] }
      options = original_options.merge(new_options).freeze
      singleton_class.silence_redefinition_of_method(:bag_options)
      define_singleton_method(:bag_options) { options }
    end
  end
end
"##;

const BAGGED_MODEL: &str = r##"class Product < ApplicationRecord
  BAG_NAMES = { price: :prices, cost: :costs }.freeze

  bags(attributes: [:price], packer: proc { 1 })

  self #: as untyped
    .configure_bags(attributes: [:cost], packer: proc { 2 })

  BAG_NAMES.keys.each do |name|
    define_method("#{name}_name") { name.to_s }
  end
end
"##;

fn bagged_app() -> emit_and_run::Overlay {
    app()
        .write("app/models/concerns/bagged.rb", MONEY_BAGS_CONCERN)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\n  include Bagged\nend\n")
        .write("app/models/product.rb", BAGGED_MODEL)
        .write("db/structure.sql", &STRUCTURE.replace("price numeric(10,2),", "price numeric(10,2),\n    cost numeric(10,2),"))
}

/// `bags`, a `self #: as untyped` call and a constant loop are all kept in
/// the class body; the macro's `attribute(name, :decimal)` names decimal
/// columns only.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_money_bag_wiring_is_replayed_on_postgresql() {
    let (dir, errors) = bagged_app().emit(BuildTarget::Ruby);
    let product = std::fs::read_to_string(dir.join("app/models/product.rb")).expect("product.rb");
    for kept in ["bags(attributes: [:price]", "configure_bags(attributes: [:cost]", "BAG_NAMES.keys.each"] {
        assert!(product.contains(kept), "{kept:?} was dropped:\n{product}\n{errors:#?}");
    }
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
}

/// A macro whose `attributes:` name a column that is not decimal keeps
/// its refusal: the declared type would not be the column's.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_bag_over_a_non_decimal_column_is_still_refused() {
    let (_dir, errors) = bagged_app()
        .write("db/structure.sql", &STRUCTURE.replace("price numeric(10,2),", "price character varying,\n    cost numeric(10,2),"))
        .emit(BuildTarget::Ruby);
    let expected = "model macro call on `Product` not lowered; `bags` reaches `attribute`, which the lowered model declares statically";
    assert!(errors.iter().any(|e| e.contains(expected)), "{errors:#?}");
}
