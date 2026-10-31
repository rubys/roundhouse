//! PostgreSQL: sti enum inheritance column, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTURE: &str = r#"CREATE TABLE sessions (
    id bigserial NOT NULL,
    session_type character varying NOT NULL,
    name character varying,
    CONSTRAINT sessions_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const STI_LOOKUP_TABLE: &str = r##"module StiLookupTable
  class SubclassNotFound < ActiveRecord::SubclassNotFound; end

  class << self
    def extended(klass)
      klass.singleton_class.attr_reader(:sti_mapping, :reverse_sti_mapping)
      klass.store_full_sti_class = false
      klass.sti_mapping = {}
    end
  end

  def sti_mapping=(mapping)
    @sti_mapping = mapping
    @reverse_sti_mapping = mapping.invert
  end

  def sti_name
    base_class.sti_mapping.fetch(name) do
      raise SubclassNotFound, "#{name} is not a known subclass in #{base_class}.sti_mapping"
    end
  end

  def sti_class_for(value)
    constant_name = base_class.reverse_sti_mapping.fetch(value) do
      raise SubclassNotFound, "#{value} is not mapped to a known subclass in #{base_class}.sti_mapping"
    end

    Object.const_get(constant_name)
  end
end
"##;

const SESSION: &str = r#"module Hosted
  class Session < ApplicationRecord
    self.table_name = "sessions"

    SESSION_TYPES = {
      payment: "payment",
      refund: "refund",
    }.freeze

    self.inheritance_column = "session_type"
    extend ::StiLookupTable
    self.sti_mapping = {
      "Hosted::PaymentSession" => SESSION_TYPES[:payment],
      "Hosted::RefundSession" => SESSION_TYPES[:refund],
    }

    enum :session_type, SESSION_TYPES
  end
end
"#;

const PAYMENT_SESSION: &str = r#"module Hosted
  class PaymentSession < Session
  end
end
"#;

const REFUND_SESSION: &str = r#"module Hosted
  class RefundSession < Session
  end
end
"#;

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/sti_lookup_table.rb", STI_LOOKUP_TABLE),
    ("app/models/hosted/session.rb", SESSION),
    ("app/models/hosted/payment_session.rb", PAYMENT_SESSION),
    ("app/models/hosted/refund_session.rb", REFUND_SESSION),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn sti_subclass_constructor_stamps_the_mapped_value_on_an_enum_column() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let payment = std::fs::read_to_string(emitted.join("app/models/hosted/payment_session.rb")).unwrap();
    assert!(payment.contains("self.session_type = Hosted::PaymentSession.sti_name"), "{payment}");
    assert!(!payment.contains("self.session_type = \"Hosted::PaymentSession\""), "{payment}");
}
