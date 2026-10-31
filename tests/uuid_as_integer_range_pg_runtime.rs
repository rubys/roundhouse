//! PostgreSQL: uuid as integer range, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE products (
    id bigserial NOT NULL,
    uuid character varying NOT NULL,
    legacy character varying,
    name character varying,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
CREATE TABLE prices (
    id bigserial NOT NULL,
    product_uuid character varying,
    CONSTRAINT prices_pkey PRIMARY KEY (id)
);
CREATE TABLE plans (
    id bigint NOT NULL,
    uuid character varying NOT NULL,
    CONSTRAINT plans_pkey PRIMARY KEY (uuid)
);
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  def self.typed_columns(*names)
    names.each { |name| attribute name.to_sym, :string }
  end
end
"#;

const PRODUCT: &str = r#"class Product < ApplicationRecord
  self.ignored_columns = %w[legacy]
  typed_columns :name, :legacy
end
"#;

const PRICE: &str = r#"class Price < ApplicationRecord
  belongs_to :product, primary_key: :uuid, foreign_key: :product_uuid, optional: true
end
"#;

const PLAN: &str = r#"class Plan < ApplicationRecord
  self.primary_key = :id
end
"#;

const PROBE: &str = r#"class PreloadProbe
  def self.run
    Price.includes(:product).to_a
  end
end
"#;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("app/models/product.rb", PRODUCT)
        .write("app/models/price.rb", PRICE)
        .write("app/models/plan.rb", PLAN)
        .write("app/models/preload_probe.rb", PROBE)
}

#[test]
#[ignore = "fork spec: postgresql"]
fn belongs_to_preload_matches_the_declared_primary_key() {
    let (emitted, _) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let price = std::fs::read_to_string(emitted.join("app/models/price.rb")).unwrap();
    let batch = price.split("def self._preload_batch_product").nth(1).expect("preload batch");
    let batch = batch.split("def _preload_product").next().unwrap();
    for expected in ["WHERE products.uuid IN (", "_attribute_serialize(\"uuid\", id)", "by_id[rec.uuid] = rec"] {
        assert!(batch.contains(expected), "missing {expected:?} in:\n{batch}");
    }
    for absent in ["products.id IN", "_attribute_serialize(\"id\"", "by_id[rec.id]"] {
        assert!(!batch.contains(absent), "unexpected {absent:?} in:\n{batch}");
    }
}

/// `self.primary_key = :id` over a table with an `id` column keys on it,
/// not on the table's PRIMARY KEY constraint (`uuid`): `id` keeps the
/// column's own reader.
#[test]
fn a_declared_id_key_is_not_aliased_to_the_constraint_key() {
    let (emitted, _) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let plan = std::fs::read_to_string(emitted.join("app/models/plan.rb")).unwrap();
    assert!(!plan.contains("def id\n        @uuid"), "id aliased to uuid:\n{plan}");
    assert!(plan.contains("def id\n"), "{plan}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_type_macro_naming_an_ignored_column_is_replayed() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        !errors.iter().any(|e| e.contains("typed_columns") && e.contains("not lowered")),
        "{}",
        errors.join("\n")
    );
    let product = std::fs::read_to_string(emitted.join("app/models/product.rb")).unwrap();
    assert!(product.contains("typed_columns :name, :legacy"), "{product}");
}

/// The runtime's `Base.attribute` takes a type-only declaration for a
/// column the compiled table leaves out, and still raises for any other
/// name it does not consult.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_runtime_attribute_accepts_an_ignored_column_only() {
    // Class-level, so no adapter: require the emitted app and run.
    let (emitted, _) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(
            r#"require File.expand_path("main", Dir.pwd)
def attempt
  yield
  puts "ok"
rescue NotImplementedError => e
  puts e.message
end
attempt { Product.attribute(:legacy, :string) }
attempt { Product.attribute(:nowhere, :string) }
attempt { Product.attribute(:legacy, :string, default: "x") }
"#,
        )
        .current_dir(&*emitted)
        .output()
        .expect("spawn ruby");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}\n{stderr}");
    assert_eq!(lines[0], "ok", "{stdout}\n{stderr}");
    assert!(lines[1].starts_with("Product.attribute(\"nowhere\""), "{stdout}");
    assert!(lines[2].starts_with("Product.attribute(\"legacy\", ...): "), "{stdout}");
}
