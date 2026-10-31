//! PostgreSQL: uniqueness validator, on the Ruby target against native Rails.
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
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);

CREATE TABLE members (
    id bigserial NOT NULL,
    shop_id bigint,
    email character varying,
    pin character varying,
    handle character varying,
    name character varying,
    code character varying,
    deleted_at timestamp without time zone,
    is_deleted boolean DEFAULT false NOT NULL,
    skip_email boolean DEFAULT false NOT NULL,
    CONSTRAINT members_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_uniqueness_validator
"#;

/// The ActiveModel::Validations port translates through I18n, which the
/// lockfile names.
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// SensitiveAttributeValidations' shape: `validates` with a lambda
/// condition put in front of the caller's.
const SENSITIVE: &str = r#"module Sensitive
  def validates_sensitive(*args)
    validates(*sensitive_args(*args))
  end

  private

  def sensitive_args(*args)
    options = args.extract_options!.dup
    options[:if] = Array(options[:if])
    options[:if].unshift(->(record) { record.name != "purged" })
    args << options
  end
end
"#;

/// The `included` hook registers through the class methods the module
/// extends, so these run at run time.
const UNIQUE_IDENTITY: &str = r#"module UniqueIdentity
  def self.included(base)
    base.extend(Sensitive)
    base.validates_sensitive :email, uniqueness: { scope: [:deleted_at, :is_deleted, :shop_id], case_sensitive: false }, allow_blank: true, allow_nil: true, unless: [:skip_email?]
    base.validates_sensitive :pin, uniqueness: { scope: :shop, case_sensitive: true }, allow_nil: true
    base.validates_uniqueness_of :handle, conditions: -> { where(is_deleted: false) }, message: "is in use", allow_nil: true
  end
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
end
"#;

const MEMBER: &str = r#"class Member < ApplicationRecord
  belongs_to :shop, optional: true
  include UniqueIdentity

  validates :name, uniqueness: { case_sensitive: false }, allow_nil: true
  validates :code, uniqueness: true, allow_nil: true
end
"#;

const PROBE: &str = r##"class UniquenessProbe
  def self.describe(label, record)
    "#{label}: #{record.valid?} #{record.errors.full_messages.inspect}"
  end

  def self.lines
    out = []
    one = Shop.create!(name: "one")
    two = Shop.create!(name: "two")
    ann = Member.create!(shop: one, email: "Ann@Example.com", pin: "ab12", handle: "ann", name: "Ann", code: "Xy")
    out << describe("email other case", Member.new(shop: one, email: "ann@example.COM"))
    out << describe("email other shop", Member.new(shop: two, email: "ann@example.com"))
    out << describe("email deleted", Member.new(shop: one, email: "ann@example.com", is_deleted: true))
    out << describe("email deleted_at", Member.new(shop: one, email: "ann@example.com", deleted_at: Time.utc(2020, 1, 2)))
    out << describe("email no shop", Member.new(email: "ann@example.com"))
    out << describe("email skipped", Member.new(shop: one, email: "ann@example.com", skip_email: true))
    out << describe("email purged", Member.new(shop: one, email: "ann@example.com", name: "purged"))
    out << describe("email nil", Member.new(shop: one, email: nil))
    out << describe("email blank", Member.new(shop: one, email: ""))
    out << describe("pin same", Member.new(shop: one, pin: "ab12"))
    out << describe("pin other case", Member.new(shop: one, pin: "AB12"))
    out << describe("pin other shop", Member.new(shop: two, pin: "ab12"))
    out << describe("handle taken", Member.new(handle: "ann"))
    out << describe("handle other case", Member.new(handle: "ANN"))
    out << describe("name other case", Member.new(name: "aNN"))
    out << describe("code same", Member.new(code: "Xy"))
    out << describe("code other case", Member.new(code: "xy"))
    out << describe("all taken", Member.new(shop: one, email: "ANN@example.com", pin: "ab12", handle: "ann", name: "ann", code: "Xy"))
    ann.email = "ann@EXAMPLE.com"
    ann.name = "ANN"
    out << describe("update self", ann)
    out << "save self: #{ann.save}"
    bob = Member.create!(shop: one, email: "bob@example.com", handle: "bob")
    bob.email = "ANN@example.com"
    bob.handle = "ann"
    out << describe("update onto other", bob)
    ann.update!(is_deleted: true)
    out << describe("handle condition", Member.new(handle: "ann"))
    begin
      Member.create!(shop: one, pin: "ab12")
    rescue ActiveRecord::RecordInvalid => e
      out << "create!: #{e.message}"
    end
    out << "shop reflection: #{Member.reflect_on_association(:shop).macro}"
    out << "validators: #{Member.validators_on(:email, :pin).map { |v| v.class.name }.inspect}"
    out << "count: #{Member.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 26] = [
    r#"email other case: false ["Email has already been taken"]"#,
    r#"email other shop: true []"#,
    r#"email deleted: true []"#,
    r#"email deleted_at: true []"#,
    r#"email no shop: true []"#,
    r#"email skipped: true []"#,
    r#"email purged: true []"#,
    r#"email nil: true []"#,
    r#"email blank: true []"#,
    r#"pin same: false ["Pin has already been taken"]"#,
    r#"pin other case: true []"#,
    r#"pin other shop: true []"#,
    r#"handle taken: false ["Handle is in use"]"#,
    r#"handle other case: true []"#,
    r#"name other case: false ["Name has already been taken"]"#,
    r#"code same: false ["Code has already been taken"]"#,
    r#"code other case: true []"#,
    r#"all taken: false ["Email has already been taken", "Pin has already been taken", "Handle is in use", "Name has already been taken", "Code has already been taken"]"#,
    r#"update self: true []"#,
    r#"save self: true"#,
    r#"update onto other: false ["Email has already been taken", "Handle is in use"]"#,
    r#"handle condition: true []"#,
    r#"create!: Validation failed: Pin has already been taken"#,
    r#"shop reflection: belongs_to"#,
    r#"validators: ["ActiveRecord::Validations::UniquenessValidator", "ActiveRecord::Validations::UniquenessValidator"]"#,
    r#"count: 2"#,
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/sensitive.rb", SENSITIVE),
    ("app/models/unique_identity.rb", UNIQUE_IDENTITY),
    ("app/models/shop.rb", SHOP),
    ("app/models/member.rb", MEMBER),
    ("app/models/uniqueness_probe.rb", PROBE),
];

const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/sensitive",
    "app/models/unique_identity",
    "app/models/shop",
    "app/models/member",
    "app/models/uniqueness_probe",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The hook's `uniqueness:` registrations are emitted as written, for the
/// runtime's UniquenessValidator; the class-body rules compile into
/// `validate`; the port ships ported, not as a load-time refusal.
#[test]
#[ignore = "fork spec: postgresql"]
fn runtime_uniqueness_is_emitted_and_ported() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let hook = read("app/models/unique_identity.rb");
    assert!(hook.contains("base.validates_sensitive :email, uniqueness:"), "{hook}");
    // The hook's literal `base.validates_uniqueness_of` compiles, beside
    // the class body's rules.
    let member = read("app/models/member.rb");
    assert!(member.contains("def validate\n"), "{member}");
    assert!(member.contains("handle_relation"), "{member}");
    assert!(member.contains("LOWER(name) = LOWER(?)"), "{member}");
    let validations = read("runtime/active_record_validations.rb");
    assert!(validations.contains("class UniquenessValidator"), "carried");
    assert!(!validations.contains("UniquenessValidator \" \\"), "no load-time refusal");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn uniqueness_validations_match_native_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "uniqueness-validator",
        &FILES,
        &emitted,
        "rh_spec_uniqueness_validator",
        "UniquenessProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
