//! PostgreSQL: abstract readonly has many scope owner, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE return_fees (
    id bigserial NOT NULL,
    order_id bigint,
    type character varying,
    CONSTRAINT return_fees_pkey PRIMARY KEY (id)
);

CREATE TABLE sales (
    id bigserial NOT NULL,
    order_id bigint,
    fee_id bigint,
    fee_type character varying,
    CONSTRAINT sales_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  def self.readonly_has_many(name, scope = nil, **options, &extension)
    readonly_scope = build_readonly_scope(scope)
    has_many(name, readonly_scope, **options.merge(autosave: false), &extension)
  end

  def self.build_readonly_scope(scope)
    if scope && scope.arity > 0
      ->(*args) {
        #: self as ActiveRecord::Relation
        readonly.instance_exec(*args, &scope)
      }
    elsif scope
      -> {
        #: self as ActiveRecord::Relation
        readonly.instance_exec(&scope)
      }
    else
      -> {
        #: self as ActiveRecord::Relation
        readonly
      }
    end
  end
end
"#;

const RETURN_FEE: &str = r#"module Returns
  module Fees
    class ReturnFee < ApplicationRecord
      abstract!

      readonly_has_many :sales,
        ->(fee) {
          scope = where(fee_type: ReturnFee.sti_class_for(fee.type))
          scope = scope.where(order_id: fee.order_id) if ::Sale.connection.adapter_name == "PostgreSQL"
          scope
        },
        class_name: 'Sale', foreign_key: :fee_id, primary_key: :id, inverse_of: false
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_namespaced_abstract_bases_owner_scoped_readonly_has_many_reaches_its_subclasses() {
    let (emitted, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("app/models/returns/fees/return_fee.rb", RETURN_FEE)
        .write(
            "app/models/returns/fees/return_shipping_fee.rb",
            "module Returns\n  module Fees\n    class ReturnShippingFee < ReturnFee\n    end\n  end\nend\n",
        )
        .write("app/models/sale.rb", "class Sale < ApplicationRecord\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    let fee = std::fs::read_to_string(emitted.join("app/models/returns/fees/return_fee.rb")).expect("return_fee.rb");
    assert!(fee.contains("def sales"), "the base must answer `sales`:\n{fee}");
}
