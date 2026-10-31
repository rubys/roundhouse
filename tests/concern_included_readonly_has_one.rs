//! PostgreSQL: concern included readonly has one, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE entities (
    id bigserial NOT NULL,
    shop_id bigint,
    CONSTRAINT entities_pkey PRIMARY KEY (id)
);

CREATE TABLE accounts (
    id bigserial NOT NULL,
    entity_id bigint,
    status character varying,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  def self.readonly_has_one(name, scope = nil, **options)
    has_one(name, scope, **options.merge(autosave: false))
  end
end
"#;

const CONCERN: &str = r#"module Ownable
  extend ActiveSupport::Concern

  included do
    with_options foreign_key: :entity_id, inverse_of: :entity do
      has_one :plain_account, class_name: "Account"
      readonly_has_one :enabled_account,
        -> { where(status: ["enabled", "configuring"]) },
        class_name: "Account"
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_concerns_readonly_has_one_under_with_options_reaches_the_includer() {
    let (emitted, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("app/models/concerns/ownable.rb", CONCERN)
        .write("app/models/entity.rb", "class Entity < ApplicationRecord\n  include Ownable\nend\n")
        .write("app/models/account.rb", "class Account < ApplicationRecord\n  belongs_to :entity\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    let entity = std::fs::read_to_string(emitted.join("app/models/entity.rb")).expect("entity.rb");
    assert!(entity.contains("def plain_account"), "{entity}");
    assert!(entity.contains("def enabled_account"), "the includer must answer `enabled_account`:\n{entity}");
    // The block's `foreign_key:` reached the macro call it wraps.
    assert!(entity.contains("entity_id"), "{entity}");
}
