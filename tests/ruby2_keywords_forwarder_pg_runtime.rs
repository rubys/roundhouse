//! PostgreSQL: ruby2 keywords forwarder, on the Ruby target against native Rails.
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
  database: rh_spec_ruby2_keywords
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const STRUCTURE: &str = r#"CREATE TABLE things (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT things_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const FORWARDER: &str = r#"module Forwarder
  def save_flagged!(*args, &block)
    @flagged = true
    save!(*args, &block)
  ensure
    @flagged = false
  end
  ruby2_keywords :save_flagged!
end
"#;

const THING: &str = r#"class Thing < ApplicationRecord
  include Forwarder

  validates :name, presence: true
end
"#;

const PROBE: &str = r##"class ForwarderProbe
  def self.lines
    out = []
    thing = Thing.new(name: "a")
    out << "keywords: #{thing.save_flagged!(validate: false)} #{thing.persisted?}"
    blank = Thing.new
    out << "validate off: #{blank.save_flagged!(validate: false)} #{blank.persisted?}"
    begin
      Thing.new.save_flagged!
    rescue ActiveRecord::RecordInvalid => e
      out << "invalid: #{e.message}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 3] = [
    "keywords: true true",
    "validate off: true true",
    "invalid: Validation failed: Name can't be blank",
];

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/forwarder",
    "app/models/thing",
    "app/models/forwarder_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/forwarder.rb", FORWARDER),
    ("app/models/thing.rb", THING),
    ("app/models/forwarder_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// The compiler side: the module body keeps its `ruby2_keywords` statement.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_module_keeps_its_ruby2_keywords_statement() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{real:?}");
    let src = std::fs::read_to_string(emitted.join("app/models/forwarder.rb")).unwrap();
    assert!(src.contains("ruby2_keywords :save_flagged!"), "{src}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_ruby2_keywords_forwarder_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{real:?}");
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "ruby2-keywords-forwarder",
        &FILES,
        &emitted,
        "rh_spec_ruby2_keywords",
        "ForwarderProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
