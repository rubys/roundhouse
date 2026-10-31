//! PostgreSQL: declared time attr, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE products (
    id bigserial NOT NULL,
    title character varying,
    opens_at time without time zone,
    closes_at time without time zone,
    starts_at time without time zone,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_declared_time_attr
"#;

const APPLICATION: &str = r#"Bundler.require(*Rails.groups)

module DeclaredTimeProbe
  class Application < Rails::Application
    config.time_zone = "Eastern Time (US & Canada)"
  end
end
"#;

/// What railties and Active Record's railtie do natively at boot for
/// that config (the driver loads no Rails application).
const NATIVE_BOOT: &str = r#"require "active_support/all"
Time.zone_default = Time.find_zone!("Eastern Time (US & Canada)")
ActiveRecord::Base.time_zone_aware_attributes = true
ActiveRecord.default_timezone = :utc
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const PRODUCT: &str = r#"class Product < ApplicationRecord
  attribute :publish_on, :time
  attribute :closes_at, :time
  attribute :starts_at, ActiveRecord::Type::Time.new
end
"#;

const PROBE: &str = r##"class DeclaredTimeAttrProbe
  def self.lines
    out = ["zone: #{Time.zone.name}"]
    %w[publish_on closes_at opens_at starts_at].each do |name|
      type = Product.type_for_attribute(name)
      cast = type.cast("10:30:15")
      out << "#{name}: #{type.type.inspect} #{type.is_a?(ActiveRecord::AttributeMethods::TimeZoneConversion::TimeZoneConverter)} #{cast.class} #{cast.utc?} #{[cast.year, cast.month, cast.day, cast.hour, cast.min, cast.sec].inspect}"
    end
    product = Product.new(title: "t", publish_on: "10:30:15", closes_at: "11:00:00", opens_at: "09:00:00", starts_at: "08:15:00")
    %w[publish_on closes_at opens_at starts_at].each do |name|
      value = product.public_send(name)
      out << "#{name} written: #{value.class} #{value.utc?} #{[value.hour, value.min, value.sec].inspect}"
    end
    product.save!
    reread = Product.find(product.id)
    %w[closes_at opens_at starts_at].each do |name|
      value = reread.public_send(name)
      out << "#{name} read: #{value.class} #{[value.year, value.month, value.day, value.hour, value.min, value.sec].inspect}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 12] = [
    "zone: Eastern Time (US & Canada)",
    "publish_on: :time true ActiveSupport::TimeWithZone false [2000, 1, 1, 10, 30, 15]",
    "closes_at: :time true ActiveSupport::TimeWithZone false [2000, 1, 1, 10, 30, 15]",
    "opens_at: :time true ActiveSupport::TimeWithZone false [2000, 1, 1, 10, 30, 15]",
    "starts_at: :time true ActiveSupport::TimeWithZone false [2000, 1, 1, 10, 30, 15]",
    "publish_on written: ActiveSupport::TimeWithZone false [10, 30, 15]",
    "closes_at written: ActiveSupport::TimeWithZone false [11, 0, 0]",
    "opens_at written: ActiveSupport::TimeWithZone false [9, 0, 0]",
    "starts_at written: ActiveSupport::TimeWithZone false [8, 15, 0]",
    "closes_at read: ActiveSupport::TimeWithZone [2000, 1, 1, 11, 0, 0]",
    "opens_at read: ActiveSupport::TimeWithZone [2000, 1, 1, 9, 0, 0]",
    "starts_at read: ActiveSupport::TimeWithZone [2000, 1, 1, 8, 15, 0]",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/application.rb", APPLICATION),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/product.rb", PRODUCT),
    ("app/models/declared_time_attr_probe.rb", PROBE),
    ("native_boot.rb", NATIVE_BOOT),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the writer of a declared type over no column casts
/// through it.
#[test]
#[ignore = "fork spec: postgresql"]
fn declared_time_writer_casts() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let product = std::fs::read_to_string(emitted.join("app/models/product.rb")).unwrap();
    let expected = r#"@publish_on = self.class._cast_column("publish_on", value)"#;
    assert!(product.contains(expected), "missing {expected:?} in:\n{product}");
}

/// The models natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn declared_time_attr_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "declared-time-attr",
        &FILES,
        &emitted,
        "rh_spec_declared_time_attr",
        "DeclaredTimeAttrProbe",
        &["native_boot", "app/models/application_record", "app/models/product", "app/models/declared_time_attr_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
