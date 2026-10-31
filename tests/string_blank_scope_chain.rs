//! PostgreSQL: string blank scope chain, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CARD: &str = r#"class Card < ApplicationRecord
  def item_known?
    if item_ref.present?
      Card.where(id: item_ref).exists?
    end
  end

  def item_missing?
    item_ref.blank? || Card.where(id: item_ref).none?
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_string_blank_beside_a_relation_chain_is_rails_blank() {
    let (emitted, _errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_spec_blank_chain\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "db/structure.sql",
            "CREATE TABLE cards (\n    id bigserial NOT NULL,\n    item_ref character varying(255),\n    CONSTRAINT cards_pkey PRIMARY KEY (id)\n);\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/card.rb", CARD)
        .emit(roundhouse::project::BuildTarget::Ruby);
    let text = std::fs::read_to_string(emitted.join("app/models/card.rb")).expect("card.rb");
    assert!(text.contains("(self.item_ref || \"\").blank?"), "{text}");
    assert!(!text.contains("strip.empty?"), "{text}");
}
