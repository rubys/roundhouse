//! PostgreSQL: store normalize extension, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    name character varying,
    settings text,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
CREATE TABLE handles (
    id bigserial NOT NULL,
    handle character varying,
    CONSTRAINT handles_pkey PRIMARY KEY (id)
);
CREATE TABLE orders (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);
CREATE TABLE adjustments (
    id bigserial NOT NULL,
    order_id bigint,
    kind character varying,
    amount integer,
    CONSTRAINT adjustments_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_store_normalize_ext
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const NATIVE_GEMS: &str = "gem \"activerecord-typedstore\"\nrequire \"active_record/typed_store\"\n";

const SHOP: &str = r##"class Shop < ApplicationRecord
  typed_store :settings do |s|
    s.string :timezone, default: "UTC"
    s.string :currency, default: "USD"
  end

  def pending_settings_keys
    settings_keys_between(settings_in_database || {}, settings)
  end

  #: (untyped, untyped) -> Array[String]
  def settings_keys_between(before, after)
    (before.keys | after.keys).map(&:to_s).uniq.reject { |key| before[key] == after[key] }.sort
  end
end
"##;

const HANDLE: &str = r##"class Handle < ApplicationRecord
  normalizes :handle, with: ->(handle) { handle.downcase }
end
"##;

const ORDER: &str = r##"class Order < ApplicationRecord
  has_many :adjustments, -> { order(id: :asc).extending(AdjustmentsExtension) }
end
"##;

const ADJUSTMENT: &str = r##"class Adjustment < ApplicationRecord
  belongs_to :order
end
"##;

const EXTENSION: &str = r##"module AdjustmentsExtension
  def refunds
    @association.owner.adjustments.select { |adjustment| adjustment.kind == "refund" }
  end

  def refund_total
    refunds.sum(&:amount)
  end

  def owner_name
    @association.owner.name
  end
end
"##;

const PROBE: &str = r##"class StoreNormalizeProbe
  def self.lines
    out = []
    shop = Shop.create!(name: "a")
    out << "created: #{shop.pending_settings_keys.inspect} #{shop.settings_in_database.keys.sort.inspect}"
    shop.currency = "CAD"
    out << "pending: #{shop.pending_settings_keys.inspect} #{shop.settings_in_database.keys.sort.inspect}"
    shop.save!
    out << "saved: #{shop.pending_settings_keys.inspect}"
    shop = Shop.find(shop.id)
    shop.timezone = "Paris"
    shop.currency = "EUR"
    out << "reloaded: #{shop.pending_settings_keys.inspect} #{shop.settings_in_database["currency"].inspect}"

    %w[Alpha BETA gamma].each { |h| Handle.create!(handle: h) }
    out << "stored: #{Handle.order(:id).pluck(:handle).inspect}"
    out << "array: #{Handle.where(handle: ["ALPHA", "Beta"]).order(:id).pluck(:handle).inspect}"
    out << "array with nil: #{Handle.where(handle: ["GAMMA", nil]).order(:id).pluck(:handle).inspect}"
    out << "scalar: #{Handle.where(handle: "BETA").pluck(:handle).inspect}"
    out << "mapped: #{Handle.where(handle: %w[ALPHA GAMMA].map { |h| h.upcase }).order(:id).pluck(:handle).inspect}"

    order = Order.create!(name: "#1")
    order.adjustments.create!(kind: "refund", amount: 5)
    order.adjustments.create!(kind: "sale", amount: 7)
    order.adjustments.create!(kind: "refund", amount: 11)
    order = Order.find(order.id)
    out << "refunds: #{order.adjustments.refunds.map(&:amount).inspect}"
    out << "total: #{order.adjustments.refund_total}"
    out << "owner: #{order.adjustments.owner_name}"
    out
  end
end
"##;

const EXPECTED: [&str; 12] = [
    r#"created: [] ["currency", "timezone"]"#,
    r#"pending: ["currency"] ["currency", "timezone"]"#,
    r#"saved: []"#,
    r#"reloaded: ["currency", "timezone"] "CAD""#,
    r#"stored: ["alpha", "beta", "gamma"]"#,
    r#"array: ["alpha", "beta"]"#,
    r#"array with nil: ["gamma"]"#,
    r#"scalar: ["beta"]"#,
    r#"mapped: ["alpha", "gamma"]"#,
    "refunds: [5, 11]",
    "total: 16",
    "owner: #1",
];

const LOADS: [&str; 8] = [
    "native/gems",
    "app/models/application_record",
    "app/models/shop",
    "app/models/handle",
    "app/models/adjustments_extension",
    "app/models/order",
    "app/models/adjustment",
    "app/models/store_normalize_probe",
];

const FILES: [(&str, &str); 12] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/handle.rb", HANDLE),
    ("app/models/adjustments_extension.rb", EXTENSION),
    ("app/models/order.rb", ORDER),
    ("app/models/adjustment.rb", ADJUSTMENT),
    ("app/models/store_normalize_probe.rb", PROBE),
    ("native/gems.rb", NATIVE_GEMS),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn store_baseline_array_normalizes_and_association_extension_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "store-normalize-ext",
        &FILES,
        &emitted,
        "rh_spec_store_normalize_ext",
        "StoreNormalizeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
