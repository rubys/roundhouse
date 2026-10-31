//! PostgreSQL: attribute api generated module, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE publications (
    id bigserial NOT NULL,
    published_at timestamp without time zone,
    CONSTRAINT publications_pkey PRIMARY KEY (id)
);
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn an_attribute_api_accessor_does_not_shadow_the_concerns_writer() {
    let (emitted, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/concerns/resource_publication.rb",
            "module ResourcePublication\n  extend ActiveSupport::Concern\n\n  included do\n    attribute :publish_on, :time\n  end\n\n  def publish_on=(value)\n    @seen = value\n  end\nend\n",
        )
        .write(
            "app/models/publication.rb",
            "class Publication < ApplicationRecord\n  include ResourcePublication\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let src = std::fs::read_to_string(emitted.join("app/models/publication.rb")).expect("publication.rb");
    let module_end = src.find("private_constant :GeneratedAttributeMethods").expect("generated module");
    let module = &src[..module_end];
    assert!(module.contains("def publish_on="), "the attribute accessor belongs to the generated module:\n{src}");
    let after = &src[module_end..];
    assert!(!after.contains("def publish_on="), "no class-body writer may shadow the concern:\n{src}");
}
