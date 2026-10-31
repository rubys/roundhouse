//! PostgreSQL: active record runtime validations, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE accounts (
    id bigserial NOT NULL,
    name character varying,
    secret character varying,
    code character varying,
    note character varying,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_runtime_validations
"#;

/// The ActiveModel::Validations port translates through I18n, which the
/// lockfile names.
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// SensitiveAttributeValidations' shape: a class-side `validate` /
/// `validates` with a lambda condition put in front of the caller's,
/// and a macro over a `validates_<kind>_of` helper.
const SENSITIVE: &str = r#"module Sensitive
  def validate_sensitive(*args, &block)
    validate(*sensitive_args(*args), &block)
  end

  def validates_sensitive(*args)
    validates(*sensitive_args(*args))
  end

  def validates_lowercase(*attributes)
    validates_format_of(*attributes, with: /\A[a-z]*\z/, allow_nil: true)
  end

  private

  def sensitive_args(*args)
    options = args.extract_options!.dup
    options[:if] = Array(options[:if])
    options[:if].unshift(->(record) { !record.purged? })
    args << options
  end
end
"#;

const PROTECTION: &str = r##"module Protection
  def self.included(base)
    base.extend(Sensitive)
    base.validate_sensitive(:secret_present, if: :secret_required?)
    base.validate_sensitive(on: :create) { errors.add(:name, :taken) if name == "taken" }
    base.validate_sensitive { |record| record.errors.add(:code, :exclusion, value: record.code) if record.code == "zz" }
    base.validates_sensitive(:code, length: { maximum: 3 }, allow_nil: true)
    base.validates_sensitive(:name, presence: true, on: :update)
    base.validates_lowercase(:code)
    base.validate_sensitive(:maybe_stop)
  end

  def secret_required?
    name != "open"
  end

  def reject
    errors.add(:base, "Rejected by hand")
    false
  end

  private

  def secret_present
    errors.add(:secret, :blank) if secret.nil?
  end

  def maybe_stop
    throw :abort if name == "stop"
  end
end
"##;

const ACCOUNT: &str = r#"class Account < ApplicationRecord
  include Protection

  validates :note, length: { maximum: 10 }

  def purged?
    note == "purged"
  end
end
"#;

const PROBE: &str = r##"class ValidationDslProbe
  def self.describe(label, record)
    "#{label}: #{record.valid?} #{record.errors.full_messages.inspect}"
  end

  def self.lines
    out = []
    out << describe("missing secret", Account.new(name: "x"))
    out << describe("open", Account.new(name: "open"))
    out << describe("purged", Account.new(name: "x", note: "purged"))
    out << describe("taken", Account.new(name: "taken", secret: "s"))
    out << describe("long code", Account.new(name: "x", secret: "s", code: "abcd"))
    out << describe("purged long code", Account.new(name: "x", note: "purged", code: "abcd"))
    out << describe("bad code", Account.new(name: "x", secret: "s", code: "AB"))
    out << describe("reserved code", Account.new(name: "x", secret: "s", code: "zz"))
    out << describe("many", Account.new(name: "taken", code: "ABCDE"))
    out << describe("both", Account.new(name: "x", note: "much too long"))
    out << describe("stop", Account.new(name: "stop", note: "much too long"))
    account = Account.new(name: "taken", secret: "s")
    out << "save new taken: #{account.save} #{account.persisted?} #{account.errors.full_messages.inspect}"
    account.name = "y"
    out << "save new: #{account.save} #{account.persisted?}"
    account.name = "taken"
    out << describe("update taken", account)
    out << "update blank: #{account.update(name: nil)} #{account.errors.full_messages.inspect}"
    account.name = "taken"
    out << "explicit create context: #{account.valid?(:create)} #{account.errors.full_messages.inspect}"
    account.name = nil
    begin
      account.save!
    rescue ActiveRecord::RecordInvalid => e
      out << "save!: #{e.class} #{e.message}"
    end
    fresh = Account.new
    out << "reject: #{fresh.reject} #{fresh.errors.full_messages.inspect}"
    out << "validators on code: #{Account.validators_on(:code).map { |v| v.class.name }.inspect}"
    # Naming ActiveRecord::Validations is what carries its port and the
    # ActiveModel::Validations port under it (project::active_model_ports_carried),
    out << "length validators on code: #{Account.validators_on(:code).grep(ActiveRecord::Validations::LengthValidator).length}"
    out << "count: #{Account.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 21] = [
    r#"missing secret: false ["Secret can't be blank"]"#,
    r#"open: true []"#,
    r#"purged: true []"#,
    r#"taken: false ["Name has already been taken"]"#,
    r#"long code: false ["Code is too long (maximum is 3 characters)"]"#,
    r#"purged long code: true []"#,
    r#"bad code: false ["Code is invalid"]"#,
    r#"reserved code: false ["Code is reserved"]"#,
    r#"many: false ["Secret can't be blank", "Name has already been taken", "Code is too long (maximum is 3 characters)", "Code is invalid"]"#,
    r#"both: false ["Secret can't be blank", "Note is too long (maximum is 10 characters)"]"#,
    r#"stop: false ["Secret can't be blank"]"#,
    r#"save new taken: false false ["Name has already been taken"]"#,
    r#"save new: true true"#,
    r#"update taken: true []"#,
    r#"update blank: false ["Name can't be blank"]"#,
    r#"explicit create context: false ["Name has already been taken"]"#,
    r#"save!: ActiveRecord::RecordInvalid Validation failed: Name can't be blank"#,
    r#"reject: false ["Rejected by hand"]"#,
    r#"validators on code: ["ActiveRecord::Validations::LengthValidator", "ActiveModel::Validations::FormatValidator"]"#,
    r#"length validators on code: 1"#,
    r#"count: 1"#,
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/sensitive.rb", SENSITIVE),
    ("app/models/protection.rb", PROTECTION),
    ("app/models/account.rb", ACCOUNT),
    ("app/models/validation_dsl_probe.rb", PROBE),
];

const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/sensitive",
    "app/models/protection",
    "app/models/account",
    "app/models/validation_dsl_probe",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The hook's registrations are emitted as written, for the runtime's
/// class methods to take; the class-body rule compiles into `validate`.
#[test]
#[ignore = "fork spec: postgresql"]
fn runtime_validations_are_emitted_as_written() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let protection = read("app/models/protection.rb");
    assert!(protection.contains("base.validate_sensitive(:secret_present, if: :secret_required?)"), "{protection}");
    assert!(protection.contains("base.validates_lowercase(:code)"), "{protection}");
    let sensitive = read("app/models/sensitive.rb");
    assert!(sensitive.contains("validate(*sensitive_args(*args), &block)"), "{sensitive}");
    let account = read("app/models/account.rb");
    assert!(account.contains("def validate\n"), "{account}");
    let connection = read("runtime/active_record/connection.rb");
    assert!(connection.contains("module RuntimeValidations"), "connection.rb carries RuntimeValidations");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn runtime_declared_validations_match_native_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "active-record-runtime-validations",
        &FILES,
        &emitted,
        "rh_spec_runtime_validations",
        "ValidationDslProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
