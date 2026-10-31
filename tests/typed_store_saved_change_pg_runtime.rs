//! PostgreSQL: typed store saved change, on the Ruby target against native Rails.
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
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_typed_store_saved
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  typed_store :settings do |s|
    s.string :timezone, default: "UTC"
    s.boolean :password_enabled, default: false
    s.integer :hsts_age, default: 0
  end

  def saved_report
    [
      saved_change_to_timezone?,
      saved_change_to_timezone,
      timezone_before_last_save,
      saved_change_to_password_enabled?,
      saved_change_to_password_enabled,
      saved_change_to_hsts_age?,
    ].inspect
  end
end
"#;

const PROBE: &str = r##"class SavedChangeProbe
  def self.lines
    out = []
    shop = Shop.create!(name: "a")
    out << "create: #{shop.saved_report}"
    shop.timezone = "Paris"
    shop.save!
    out << "timezone: #{shop.saved_report}"
    shop.password_enabled = true
    shop.save!
    out << "password: #{shop.saved_report}"
    shop.name = "b"
    shop.save!
    out << "name only: #{shop.saved_report}"
    Shop.find(shop.id).tap { |s| s.update!(hsts_age: 5) }.then { |s| out << "reloaded: #{s.saved_report}" }
    out
  end
end
"##;

/// Native only: the typed_store gem, which the emitted tree's runtime
/// replaces.
const TYPED_STORE_GEM: &str = "gem \"activerecord-typedstore\"\nrequire \"active_record/typed_store\"\n";

const EXPECTED: [&str; 5] = [
    r#"create: [false, nil, "UTC", false, nil, false]"#,
    r#"timezone: [true, ["UTC", "Paris"], "UTC", false, nil, false]"#,
    r#"password: [false, nil, "Paris", true, [false, true], false]"#,
    r#"name only: [false, nil, nil, false, nil, false]"#,
    r#"reloaded: [false, nil, "Paris", false, nil, true]"#,
];

const LOADS: [&str; 4] = [
    "native/typed_store_gem",
    "app/models/application_record",
    "app/models/shop",
    "app/models/saved_change_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/saved_change_probe.rb", PROBE),
    ("native/typed_store_gem.rb", TYPED_STORE_GEM),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn a_typed_store_keys_saved_change_readers_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "typed-store-saved",
        &FILES,
        &emitted,
        "rh_spec_typed_store_saved",
        "SavedChangeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

