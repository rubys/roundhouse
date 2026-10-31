//! PostgreSQL: relation find with block keeps nil, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE item_states (
    id bigserial NOT NULL,
    line_id bigint,
    quantity bigint,
    CONSTRAINT item_states_pkey PRIMARY KEY (id)
);
"#;

const HANDLER: &str = r#"class Handler
  class << self
    #: (Hash[Integer, Array[ItemState]], Integer) -> Array[ItemState]
    def by_line(by_id, line)
      states = by_id[1] || []
      state = states.find { |s| s.line_id == line } || states.find { |s| s.line_id.nil? }
      out = []
      out << state if state.present?
      out
    end

    #: (Integer) -> ItemState
    def by_id(id)
      ItemState.find(id)
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_block_find_over_models_stays_nilable_and_the_id_finder_does_not() {
    let (emitted, _errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/item_state.rb", "class ItemState < ApplicationRecord\nend\n")
        .write("app/models/handler.rb", HANDLER)
        .emit(roundhouse::project::BuildTarget::Ruby);
    let text = std::fs::read_to_string(emitted.join("app/models/handler.rb")).unwrap();
    assert!(!text.contains("if true"), "present? folded to a constant:\n{text}");
    assert!(
        text.contains("out << state if !(state.nil?)"),
        "the nil guard on the block find must stay:\n{text}"
    );
    assert!(text.contains("ItemState.find(id)"), "the id finder is untouched:\n{text}");
}
