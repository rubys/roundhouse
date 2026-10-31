//! PostgreSQL: model init sti self parent, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_model_init_sti
"#;

const STRUCTURE: &str = r#"CREATE TABLE payments (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    CONSTRAINT payments_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const TILE_MODEL: &str = r#"module Tiles
  class TileModel
    include ActiveModel::Model
    include ActiveModel::Attributes

    attribute :style, :string
  end
end
"#;

const FEE_TILE: &str = r#"module Tiles
  class FeeTile < TileModel
    attribute :amount, :integer
  end
end
"#;

const PAYMENT: &str = r#"class Payment < ApplicationRecord
  class Card < self
    def brand
      "visa"
    end
  end

  class Local < self
  end

  class Konbini < Local
  end
end
"#;

fn overlay(files: &[(&str, &str)]) -> emit_and_run::Overlay {
    [
        ("roundhouse.yml", ""),
        ("Gemfile.lock", LOCK),
        ("config/database.yml", DATABASE_YML),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", STRUCTURE),
        ("app/models/application_record.rb", APPLICATION_RECORD),
    ]
    .iter()
    .chain(files)
    .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn emit(files: &[(&str, &str)]) -> emit_and_run::Emitted {
    let (emitted, errors) = overlay(files).emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{real:?}");
    emitted
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_attributes_model_without_initialize_takes_the_attribute_hash() {
    let emitted = emit(&[("app/models/tiles/tile_model.rb", TILE_MODEL), ("app/models/tiles/fee_tile.rb", FEE_TILE)]);
    let src = std::fs::read_to_string(emitted.join("app/models/tiles/tile_model.rb")).unwrap();
    assert!(src.contains("include ActiveModel::Attributes"), "{src}");
    let init = src.split("def initialize(attributes = {})").nth(1).unwrap_or_else(|| panic!("no attribute-hash initialize:\n{src}"));
    let super_at = init.find("super()").unwrap_or_else(|| panic!("no super():\n{src}"));
    let assign_at = init.find("assign_attributes(attributes)").unwrap_or_else(|| panic!("no assign_attributes:\n{src}"));
    assert!(super_at < assign_at, "the attribute set is seeded before the assignment:\n{src}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_nested_class_below_self_is_a_subclass_of_the_enclosing_model() {
    let emitted = emit(&[("app/models/payment.rb", PAYMENT)]);
    let mut all = std::fs::read_to_string(emitted.join("app/models/payment.rb")).unwrap_or_default();
    for entry in std::fs::read_dir(emitted.join("app/models/payment")).unwrap() {
        all.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
    }
    assert!(!all.is_empty(), "no emitted payment sources under {}", emitted.display());
    assert!(all.contains("class Card < Payment"), "Card is not below Payment:\n{all}");
    assert!(all.contains("class Local < Payment"), "Local is not below Payment:\n{all}");
    assert!(!all.contains("class Card\n"), "bare `class Card` (an Object subclass):\n{all}");
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn class_create_with_an_array_creates_one_record_each() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel/cruby/active_record_callbacks.rb"),
    )
    .unwrap();
    for method in ["create!", "create"] {
        let def = format!("def self.{method}(attrs = {{}}, &block)\n      return attrs.map {{ |each_attrs| {method}(each_attrs, &block) }} if attrs.is_a?(Array)");
        assert!(src.contains(&def), "{method} lacks the Array form");
    }
}
