//! PostgreSQL: active model initialize super, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_am_initialize_super
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const STRUCTURE: &str = r#"CREATE TABLE orders (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);
"#;

const ORDER: &str = "class Order < ApplicationRecord\nend\n";

const LINE_ITEM: &str = r#"class Order
  class LineItem
    include ActiveModel::Model

    attr_accessor :sku, :quantity, :loaded

    def initialize(*args)
      super
      load_defaults!
    end

    def load_defaults!
      self.quantity ||= 1
      self.loaded = true
    end
  end
end
"#;

const PROBE: &str = r##"class InitializeSuperProbe
  def self.lines
    out = []
    item = Order::LineItem.new(sku: "a")
    out << "new: #{item.sku} #{item.quantity} #{item.loaded} #{item.persisted?}"
    out << "explicit: #{Order::LineItem.new(sku: "b", quantity: 4).quantity}"
    out << "empty: #{Order::LineItem.new.quantity}"
    begin
      Order::LineItem.new(nope: 1)
    rescue ActiveModel::UnknownAttributeError => e
      out << "unknown: #{e.message}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 4] = [
    "new: a 1 true false",
    "explicit: 4",
    "empty: 1",
    "unknown: unknown attribute 'nope' for Order::LineItem.",
];

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/order",
    "app/models/order/line_item",
    "app/models/initialize_super_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/order.rb", ORDER),
    ("app/models/order/line_item.rb", LINE_ITEM),
    ("app/models/initialize_super_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// The compiler side: the include stands next to the class's own `initialize`.
#[test]
#[ignore = "fork spec: postgresql"]
fn an_own_initialize_calling_super_keeps_the_active_model_include() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{real:?}");
    let src = std::fs::read_to_string(emitted.join("app/models/order/line_item.rb")).unwrap();
    assert!(src.contains("include ActiveModel::Model"), "{src}");
    assert!(emitted.join("runtime/active_model_api.rb").exists(), "no runtime/active_model_api.rb");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_own_initialize_calling_super_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{real:?}");
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "am-initialize-super",
        &FILES,
        &emitted,
        "rh_spec_am_initialize_super",
        "InitializeSuperProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
