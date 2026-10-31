//! PostgreSQL: store accessor, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::project::BuildTarget;

const STRUCTURE: &str = r#"CREATE TABLE accounts (
    id bigserial NOT NULL,
    name character varying,
    settings text,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = "test:\n  adapter: postgresql\n  encoding: unicode\n  database: rh_spec_store_accessor\n";
const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

/// An app coder (any object answering `load` and `dump`): the column holds
/// JSON, and a nil column loads as an empty Hash.
const SETTINGS_CODER: &str = r##"require "json"

module SettingsCoder
  def self.load(text)
    text.nil? || text.empty? ? {} : JSON.parse(text)
  end

  def self.dump(value)
    JSON.generate(value)
  end
end
"##;

const ACCOUNT: &str = r##"class Account < ApplicationRecord
  serialize :settings, coder: SettingsCoder
  store_accessor :settings, :theme, :font
  store_accessor :settings, [:locale], prefix: true
  store_accessor :settings, :zone, suffix: :tz
end
"##;

const PROBE: &str = r##"class StoreProbe
  def self.lines
    out = []
    a = Account.new(name: "a")
    out << "new: #{a.theme.inspect} #{a.font.inspect} #{a.settings.inspect}"
    a.theme = "dark"
    a.settings_locale = "en"
    a.zone_tz = "UTC"
    out << "set: #{a.theme.inspect} #{a.settings.to_h.inspect} #{a.theme_changed?} #{a.font_changed?} #{a.theme_was.inspect} #{a.theme_change.inspect}"
    a.save!
    out << "saved: #{a.saved_change_to_theme?} #{a.saved_change_to_font?} #{a.saved_change_to_theme.inspect} #{a.theme_changed?}"
    found = Account.find(a.id)
    out << "found: #{found.theme.inspect} #{found.settings_locale.inspect} #{found.zone_tz.inspect} #{found.settings.to_h.inspect}"
    found.theme = "light"
    out << "update: #{found.theme_changed?} #{found.theme_was.inspect} #{found.theme_change.inspect} #{found.font_changed?}"
    found.theme = "dark"
    out << "reverted: #{found.theme_changed?}"
    found.font = "mono"
    found.save!
    out << "second save: #{found.saved_change_to_font?} #{found.saved_change_to_theme?} #{found.theme_before_last_save.inspect} #{found.font_before_last_save.inspect}"
    out << "reloaded: #{Account.find(a.id).font.inspect}"
    out << "stored: #{Account.stored_attributes.inspect}"
    out
  end
end
"##;

const EXPECTED: &[&str] = &[
    "new: nil nil {}",
    r#"set: "dark" {"theme" => "dark", "locale" => "en", "zone" => "UTC"} true false nil [nil, "dark"]"#,
    r#"saved: true false [nil, "dark"] false"#,
    r#"found: "dark" "en" "UTC" {"theme" => "dark", "locale" => "en", "zone" => "UTC"}"#,
    r#"update: true "dark" ["dark", "light"] false"#,
    "reverted: false",
    r#"second save: true false "dark" nil"#,
    r#"reloaded: "mono""#,
    r#"stored: {settings: [:theme, :font, :locale, :zone]}"#,
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/settings_coder.rb", SETTINGS_CODER),
    ("app/models/account.rb", ACCOUNT),
    ("app/models/store_probe.rb", PROBE),
];

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/settings_coder",
    "app/models/account",
    "app/models/store_probe",
];

#[test]
#[ignore = "fork spec: postgresql"]
fn store_accessor_matches_native_rails_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "store-accessor",
        &FILES,
        &emitted,
        "rh_spec_store_accessor",
        "StoreProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
