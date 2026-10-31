//! PostgreSQL: macro constant option replay, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE orders (
    id bigserial NOT NULL,
    total_price_usd numeric,
    amount numeric,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  def self.priced(*columns, currency: nil, currency_column: nil)
    columns.flatten.each do |column|
      column_string = column.to_s.freeze
      attribute(column_string, :decimal)
    end
  end
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
  USD_CURRENCY_CODE = "USD"

  priced :total_price_usd, currency: USD_CURRENCY_CODE
  priced :amount, currency_column: :currency
end
"#;

fn emitted_order() -> String {
    let app = [
        ("roundhouse.yml", ""),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", STRUCTURE),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/order.rb", ORDER),
    ]
    .iter()
    .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, _) = app.emit(roundhouse::project::BuildTarget::Ruby);
    std::fs::read_to_string(emitted.join("app/models/order.rb")).expect("order.rb emitted")
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_constant_option_does_not_stop_the_macro_replay() {
    let out = emitted_order();
    assert!(out.contains("priced :amount, currency_column: :currency"), "{out}");
    assert!(out.contains("priced :total_price_usd, currency: Order::USD_CURRENCY_CODE"), "{out}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_constant_option_under_a_shadowed_namespace_is_rooted() {
    let claim = r#"module Shipping
  class Claim < ApplicationRecord
    self.table_name = "orders"
    USD_CURRENCY_CODE = "USD"

    priced :total_price_usd, currency: USD_CURRENCY_CODE
  end
end
"#;
    let app = [
        ("roundhouse.yml", ""),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", STRUCTURE),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/shipping/claim.rb", claim),
        ("app/models/shipping/shipping.rb", "module Shipping\n  class Shipping\n  end\nend\n"),
    ]
    .iter()
    .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, _) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let out = std::fs::read_to_string(emitted.join("app/models/shipping/claim.rb")).expect("claim.rb emitted");
    assert!(out.contains("priced :total_price_usd, currency: ::Shipping::Claim::USD_CURRENCY_CODE"), "{out}");
}
