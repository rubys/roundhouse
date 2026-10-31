//! PostgreSQL: active model validations declared type, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE variants (
    id bigserial NOT NULL,
    sku character varying,
    CONSTRAINT variants_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_validator_type
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const VARIANT: &str = r#"class Variant < ApplicationRecord
  #: -> ActiveModel::Validations::LengthValidator
  def size_validator
    @size_validator ||= ActiveModel::Validations::LengthValidator.new(
      attributes: [:sku], maximum: 3
    ) #: ActiveModel::Validations::LengthValidator?
  end

  def sku_size_errors
    size_validator.validate_each(self, :sku, sku)
    errors.full_messages
  end
end
"#;

const PROBE: &str = r##"class ValidatorTypeProbe
  def self.lines
    [Variant.new(sku: "ab").sku_size_errors.inspect, Variant.new(sku: "abcd").sku_size_errors.inspect]
  end
end
"##;

const FILES: [(&str, &str); 8] = [
    ("Gemfile.lock", LOCK),
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/variant.rb", VARIANT),
    ("app/models/validator_type_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_declared_validator_type_is_carried_not_refused() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let variant = std::fs::read_to_string(emitted.join("app/models/variant.rb")).expect("emitted variant.rb");
    assert!(!variant.contains("not supported"), "{variant}");
    assert!(variant.contains("ActiveModel::Validations::LengthValidator.new"), "{variant}");
    assert!(emitted.join("runtime/active_model_validations.rb").exists(), "port not carried");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_declared_validator_type_answers_as_natively_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "validator-declared-type",
        &FILES,
        &emitted,
        "rh_spec_validator_type",
        "ValidatorTypeProbe",
        &["app/models/application_record", "app/models/variant", "app/models/validator_type_probe"],
    ) else { return };
    assert_eq!(native, ["[]", "[\"Sku is too long (maximum is 3 characters)\"]"], "native");
    assert_eq!(emitted_lines, native, "emitted");
}
