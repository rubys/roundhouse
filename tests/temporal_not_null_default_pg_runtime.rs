//! PostgreSQL: temporal not null default, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE prices (
    id bigserial NOT NULL,
    name character varying,
    active_at timestamp(6) without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    inactive_at timestamp(6) without time zone,
    CONSTRAINT prices_pkey PRIMARY KEY (id)
);
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const PRICE: &str = "class Price < ApplicationRecord\nend\n";

const PROBE: &str = r##"class PriceProbe
  def self.lines
    out = []
    price = Price.create!(name: "basic")
    out << "after create: #{price.persisted?} #{price.inactive_at.inspect}"
    stored = Price.find(price.id)
    out << "stored: #{stored.active_at.is_a?(Time)} #{stored.inactive_at.inspect}"
    explicit = Price.create!(name: "dated", active_at: Time.utc(2020, 1, 2, 3, 4, 5))
    out << "explicit: #{Price.find(explicit.id).active_at.utc.iso8601}"
    out
  end
end
"##;

const DATABASE: &str = "rh_spec_temporal_not_null_default";

fn files() -> Vec<(String, String)> {
    vec![
        ("roundhouse.yml".into(), "".into()),
        ("Gemfile.lock".into(), LOCK.into()),
        (
            "config/database.yml".into(),
            format!("default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: {DATABASE}\n"),
        ),
        ("config/routes.rb".into(), "Rails.application.routes.draw do\nend\n".into()),
        ("db/structure.sql".into(), STRUCTURE.into()),
        ("app/models/application_record.rb".into(), APPLICATION_RECORD.into()),
        ("app/models/price.rb".into(), PRICE.into()),
        ("app/models/price_probe.rb".into(), PROBE.into()),
    ]
}

fn emit() -> (emit_and_run::Emitted, Vec<String>) {
    let app = files().iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    app.emit(roundhouse::project::BuildTarget::Ruby)
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_insert_leaves_a_blank_not_null_timestamp_to_the_column_default() {
    let (emitted, errors) = emit();
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let price = std::fs::read_to_string(emitted.join("app/models/price.rb")).expect("price.rb");
    assert!(price.contains("Db.escape_temporal(@active_at_raw)"), "{price}");
    // A nullable timestamp keeps NULL, and the updated row is not affected.
    assert!(price.contains("Db.escape_string_opt(@inactive_at_raw)"), "{price}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_blank_not_null_timestamp_takes_the_column_default_on_create() {
    let (emitted, errors) = emit();
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let files = files();
    let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let loads = ["app/models/application_record", "app/models/price", "app/models/price_probe"];
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("temporal-not-null-default", &files, &emitted, DATABASE, "PriceProbe", &loads) else { return };
    assert_eq!(
        native,
        [
            "after create: true nil",
            "stored: true nil",
            "explicit: 2020-01-02T03:04:05Z",
        ]
    );
    assert_eq!(emitted_lines, native, "emitted");
}
