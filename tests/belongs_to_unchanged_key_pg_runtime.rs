//! PostgreSQL: belongs to unchanged key, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE agreements (
    id bigserial NOT NULL,
    version integer,
    CONSTRAINT agreements_pkey PRIMARY KEY (id)
);

CREATE TABLE enrollments (
    id bigserial NOT NULL,
    agreement_id bigint,
    note character varying,
    CONSTRAINT enrollments_pkey PRIMARY KEY (id)
);
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_belongs_to_unchanged_key
"#;

const APPLICATION: &str = "require \"rails\"\n\nmodule Probe\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n";

/// The native side has no application.rb to read; it sets what
/// `load_defaults 8.1` sets before any model declares a `belongs_to`.
const APPLICATION_RECORD: &str = r#"if ActiveRecord.respond_to?(:belongs_to_required_validates_foreign_key=)
  ActiveRecord.belongs_to_required_validates_foreign_key = false
end

class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
  self.belongs_to_required_by_default = true
end
"#;

const AGREEMENT: &str = "class Agreement < ApplicationRecord\nend\n";

const ENROLLMENT: &str = "class Enrollment < ApplicationRecord\n  belongs_to :agreement\nend\n";

const PROBE: &str = r##"class UnchangedKeyProbe
  def self.lines
    out = []
    agreement = Agreement.create!(version: 1)
    dangling = Enrollment.insert_all!([{ agreement_id: agreement.id + 1000, note: "a" }])
    enrollment = Enrollment.find_by!(note: "a")
    out << "unchanged dangling key: #{enrollment.update(note: "b")} #{enrollment.errors.full_messages.inspect}"
    enrollment.agreement_id = agreement.id + 2000
    out << "changed to a missing row: #{enrollment.save} #{enrollment.errors.full_messages.inspect}"
    enrollment.agreement_id = agreement.id
    out << "changed to a row: #{enrollment.save}"
    out << "new without a key: #{Enrollment.new.save}"
    out << "new with a missing row: #{Enrollment.new(agreement_id: agreement.id + 3000).save}"
    out << "new with a row: #{Enrollment.new(agreement: agreement).save}"
    out
  end
end
"##;

const EXPECTED: [&str; 6] = [
    "unchanged dangling key: true []",
    "changed to a missing row: false [\"Agreement must exist\"]",
    "changed to a row: true",
    "new without a key: false",
    "new with a missing row: false",
    "new with a row: true",
];

const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/agreement",
    "app/models/enrollment",
    "app/models/unchanged_key_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/application.rb", APPLICATION),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/agreement.rb", AGREEMENT),
    ("app/models/enrollment.rb", ENROLLMENT),
    ("app/models/unchanged_key_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_unchanged_foreign_key_skips_the_existence_check_like_native_rails() {
    let (emitted, _) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "belongs-to-unchanged-key",
        &FILES,
        &emitted,
        "rh_spec_belongs_to_unchanged_key",
        "UnchangedKeyProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}

/// The guard in the emitted `validate`, without PostgreSQL: present under
/// `load_defaults` 8.1, absent without it.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_existence_check_is_guarded_by_the_load_defaults() {
    let (emitted, _) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let enrollment = std::fs::read_to_string(emitted.join("app/models/enrollment.rb")).unwrap();
    assert!(enrollment.contains("attribute_changed?(\"agreement_id\")"), "{enrollment}");

    let (emitted, _) = FILES
        .iter()
        .filter(|(path, _)| *path != "config/application.rb")
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
        .emit(roundhouse::project::BuildTarget::Ruby);
    let enrollment = std::fs::read_to_string(emitted.join("app/models/enrollment.rb")).unwrap();
    assert!(!enrollment.contains("attribute_changed?(\"agreement_id\")"), "{enrollment}");
}
