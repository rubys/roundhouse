//! PostgreSQL: sti typed store, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE credentials (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    data text,
    sensitive_data text,
    extra_data text,
    CONSTRAINT credentials_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sti_typed_store
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const CREDENTIAL: &str = r#"class Credential < ApplicationRecord
  include RedactableStore

  def billing_agreement_id
    raise NotImplementedError, "Abstract method called"
  end
end
"#;

const SHARED_AGREEMENT: &str = r#"module SharedAgreement
  extend ActiveSupport::Concern

  included do
    typed_store :data do |s|
      s.string :billing_agreement_id
      s.integer :attempts, default: 0
    end

    serialize :sensitive_data, type: Hash, coder: JSON
    validates :name, presence: true
  end
end
"#;

const AGREEMENT_CREDENTIAL: &str = r#"class AgreementCredential < Credential
  include SharedAgreement
end
"#;

const TOKEN_CREDENTIAL: &str = r#"class TokenCredential < Credential
  typed_store :data do |s|
    s.string :token
  end

  serialize :sensitive_data, type: Hash, coder: JSON
  validates :token, presence: true
end
"#;

const REDACTABLE_STORE: &str = r#"module RedactableStore
  extend ActiveSupport::Concern

  class_methods do
    def redactable_typed_store(store_attribute, options = {}, &block)
      typed_store(store_attribute, options, &block)

      @redactable_typed_stores ||= {}
      @redactable_typed_stores[store_attribute] ||= {}
      typed_stores[store_attribute].fields.each do |name, field|
        @redactable_typed_stores[store_attribute][name] = field.type_sym
      end
    end

    def redactable_typed_stores
      @redactable_typed_stores
    end
  end
end
"#;

const SHARED_STORE: &str = r#"module SharedStore
  extend ActiveSupport::Concern
  include RedactableStore
end

module SharedStoreBody
  extend ActiveSupport::Concern

  included do
    redactable_typed_store :extra_data do |s|
      s.string :body_card_id
    end
  end
end
"#;

const BODY_CARD_CREDENTIAL: &str = r#"class BodyCardCredential < Credential
  include SharedStoreBody
end
"#;

const VAULT_CARD: &str = r#"module VaultCard
  extend ActiveSupport::Concern

  included do
    redactable_typed_store :data do |s|
      s.string :card_id
    end

    serialize :sensitive_data, type: Hash, coder: JSON
    validates :sensitive_data, presence: false
  end
end
"#;

const CARD_CREDENTIAL: &str = r#"class CardCredential < Credential
  include VaultCard
end
"#;

const PROBE: &str = r##"class StiStoreProbe
  def self.lines
    out = []
    a = AgreementCredential.new(name: "a", billing_agreement_id: "B-1")
    out << "new: #{a.billing_agreement_id.inspect} #{a.attempts.inspect} #{a.sensitive_data.inspect} #{a.valid?}"
    a.attempts = "3"
    a.sensitive_data = { "k" => 1 }
    a.save!
    found = Credential.find(a.id)
    out << "found: #{found.class} #{found.billing_agreement_id.inspect} #{found.attempts.inspect} #{found.sensitive_data.inspect}"
    blank = AgreementCredential.new
    out << "invalid: #{blank.valid?} #{blank.errors.full_messages.inspect}"
    t = TokenCredential.new(name: "t")
    out << "token invalid: #{t.valid?} #{t.errors.full_messages.inspect} #{t.respond_to?(:billing_agreement_id)}"
    t.token = "tok"
    t.save!
    out << "token: #{Credential.find(t.id).token.inspect} #{TokenCredential.find(t.id).sensitive_data.inspect}"
    b = BodyCardCredential.create!(name: "b", body_card_id: "body-1")
    out << "body: #{Credential.find(b.id).body_card_id.inspect} #{BodyCardCredential.redactable_typed_stores.inspect}"
    c = CardCredential.create!(name: "c", card_id: "card-1")
    out << "card: #{Credential.find(c.id).card_id.inspect} #{c.valid?} #{CardCredential.redactable_typed_stores.inspect} #{AgreementCredential.redactable_typed_stores.inspect}"
    out << "base: #{Credential.new.respond_to?(:token)} #{(begin; Credential.new.billing_agreement_id; rescue NotImplementedError; $!.class; end)}"
    out
  end
end
"##;

/// Native only: the typed_store gem, which the emitted tree's runtime
/// replaces.
const TYPED_STORE_GEM: &str = "gem \"activerecord-typedstore\"\nrequire \"active_record/typed_store\"\n";

const EXPECTED: [&str; 8] = [
    r#"new: "B-1" 0 {} true"#,
    r#"found: AgreementCredential "B-1" 3 {"k" => 1}"#,
    r#"invalid: false ["Name can't be blank"]"#,
    r#"token invalid: false ["Token can't be blank"] true"#,
    r#"token: "tok" {}"#,
    r#"body: "body-1" {extra_data: {body_card_id: :string}}"#,
    r#"card: "card-1" true {data: {card_id: :string}} nil"#,
    "base: false NotImplementedError",
];

/// What native Rails requires, in order.
const LOADS: [&str; 12] = [
    "native/typed_store_gem",
    "app/models/application_record",
    "app/models/redactable_store",
    "app/models/shared_store",
    "app/models/credential",
    "app/models/shared_agreement",
    "app/models/agreement_credential",
    "app/models/token_credential",
    "app/models/vault_card",
    "app/models/card_credential",
    "app/models/body_card_credential",
    "app/models/sti_store_probe",
];

const FILES: [(&str, &str); 16] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/credential.rb", CREDENTIAL),
    ("app/models/shared_agreement.rb", SHARED_AGREEMENT),
    ("app/models/agreement_credential.rb", AGREEMENT_CREDENTIAL),
    ("app/models/token_credential.rb", TOKEN_CREDENTIAL),
    ("app/models/redactable_store.rb", REDACTABLE_STORE),
    ("app/models/vault_card.rb", VAULT_CARD),
    ("app/models/card_credential.rb", CARD_CREDENTIAL),
    ("app/models/shared_store.rb", SHARED_STORE),
    ("app/models/body_card_credential.rb", BODY_CARD_CREDENTIAL),
    ("app/models/sti_store_probe.rb", PROBE),
    ("native/typed_store_gem.rb", TYPED_STORE_GEM),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: nothing is dropped or refused. Each subclass body
/// runs its `typed_store` and `serialize` at load (the Concern's
/// statements carried into the includer) and lowers its validations
/// into its `validate`; the model compiles the serialized column on the
/// path that consults the subclass's declared type.
#[test]
#[ignore = "fork spec: postgresql"]
fn sti_subclass_store_serialize_and_validations_are_lowered() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let agreement = std::fs::read_to_string(emitted.join("app/models/agreement_credential.rb")).unwrap();
    for expected in [
        "  typed_store :data do |s|",
        "s.string :billing_agreement_id",
        "  serialize :sensitive_data, type: Hash, coder: JSON\n",
        "errors << \"Name can't be blank\"",
    ] {
        assert!(agreement.contains(expected), "missing {expected:?} in:\n{agreement}");
    }
    let token = std::fs::read_to_string(emitted.join("app/models/token_credential.rb")).unwrap();
    for expected in [
        "  typed_store :data do |s|",
        "s.string :token",
        "  serialize :sensitive_data, type: Hash, coder: JSON\n",
        "errors << \"Token can't be blank\"",
    ] {
        assert!(token.contains(expected), "missing {expected:?} in:\n{token}");
    }
    let credential = std::fs::read_to_string(emitted.join("app/models/credential.rb")).unwrap();
    assert!(credential.contains("def self._retypable_attribute_names"), "credential:\n{credential}");
    let retypable = credential.split("def self._retypable_attribute_names").nth(1).unwrap();
    let retypable = retypable.split("\n  end").next().unwrap();
    assert!(retypable.contains("\"data\""), "data is not retypable:\n{credential}");
    assert!(retypable.contains("\"extra_data\""), "extra_data is not retypable:\n{credential}");
}

/// The models natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql native control"]
fn sti_typed_store_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-typed-store",
        &FILES,
        &emitted,
        "rh_spec_sti_typed_store",
        "StiStoreProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
