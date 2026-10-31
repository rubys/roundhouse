//! PostgreSQL: readonly has one through class, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);

CREATE TABLE entities (
    id bigserial NOT NULL,
    shop_id bigint,
    CONSTRAINT entities_pkey PRIMARY KEY (id)
);

CREATE TABLE payments_accounts (
    id bigserial NOT NULL,
    shop_id bigint,
    entity_id bigint,
    CONSTRAINT payments_accounts_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  def self.readonly_has_one(name, scope = nil, **options)
    has_one(name, scope, **options.merge(autosave: false))
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_readonly_has_one_through_reads_the_source_associations_class() {
    let (emitted, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write(
            "app/models/shop.rb",
            "class Shop < ApplicationRecord\n  has_many :entities\n  has_one :primary_entity, class_name: \"Entity\"\n  readonly_has_one :payments_account, through: :primary_entity\nend\n",
        )
        .write(
            "app/models/entity.rb",
            "class Entity < ApplicationRecord\n  belongs_to :shop\n  readonly_has_one :payments_account, class_name: \"::Payments::Account\", foreign_key: :entity_id\nend\n",
        )
        .write(
            "app/models/payments/account.rb",
            "module Payments\n  class Account < ApplicationRecord\n    self.table_name = \"payments_accounts\"\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let shop = std::fs::read_to_string(emitted.join("app/models/shop.rb")).expect("shop.rb");
    let reader = shop.split("def payments_account\n").nth(1).expect("Shop#payments_account").split("\n    end").next().unwrap();
    assert!(reader.contains("Payments::Account"), "the through reader must query the source's class:\n{reader}");
    assert!(!reader.contains("PaymentsAccount"), "the name-derived class is not a model:\n{reader}");
}
