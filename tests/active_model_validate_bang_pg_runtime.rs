//! PostgreSQL: active model validate bang, on the Ruby target against native Rails.
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
  database: rh_spec_active_model_validate_bang
"#;

/// The ActiveModel::Validations port translates through I18n, which the
/// lockfile names.
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// RequestForInformation's shape: `validate!` in `initialize`.
const REQUEST: &str = r#"module Payments
  class Request
    include ActiveModel::Validations

    PATTERN = /\AREQUEST_FOR_INFORMATION_\d+\z/

    attr_reader :request_name, :category

    validates :request_name, presence: true, format: { with: PATTERN }

    def initialize(request_name, category)
      @request_name = request_name
      @category = category
      validate!
    end
  end
end
"#;

/// A rule `on:` a context: `validate!(context)` runs it.
const TICKET: &str = r#"class Ticket
  include ActiveModel::Validations

  attr_accessor :seat

  validates :seat, presence: true, on: :booking

  def initialize(seat)
    @seat = seat
  end
end
"#;

const PROBE: &str = r##"class VbProbe
  def self.attempt
    yield.inspect
  rescue ActiveModel::ValidationError => e
    "#{e.class.name}: #{e.message}"
  end

  def self.lines
    out = []
    out << "ok: #{attempt { Payments::Request.new("REQUEST_FOR_INFORMATION_221", "LEGAL").request_name }}"
    out << "blank: #{attempt { Payments::Request.new(nil, "LEGAL") }}"
    out << "pattern: #{attempt { Payments::Request.new("INVALID_REQUEST_NAME", "LEGAL") }}"
    record = Payments::Request.allocate
    record.instance_variable_set(:@request_name, "REQUEST_FOR_INFORMATION_7")
    out << "bang true: #{record.validate!} #{record.invalid?}"
    begin
      record.instance_variable_set(:@request_name, "nope")
      record.validate!
    rescue ActiveModel::ValidationError => e
      out << "model: #{e.model.equal?(record)} #{record.errors.full_messages.inspect}"
    end
    ticket = Ticket.new(nil)
    out << "no context: #{attempt { ticket.validate! }}"
    out << "context: #{attempt { ticket.validate!(:booking) }}"
    out << "seated: #{attempt { Ticket.new("A1").validate!(:booking) }}"
    # `validate(context)` is `valid?(context)`; nil names a context too.
    unseated = Ticket.new(nil)
    out << "validate context: #{unseated.validate(:booking)} #{unseated.errors.full_messages.inspect}"
    out << "validate seated: #{Ticket.new("A1").validate(:booking)}"
    out << "validate nil: #{Ticket.new(nil).validate(nil)} #{Ticket.new(nil).validate(nil).class}"
    out
  end
end
"##;

const EXPECTED: [&str; 11] = [
    r#"ok: "REQUEST_FOR_INFORMATION_221""#,
    r#"blank: ActiveModel::ValidationError: Validation failed: Request name can't be blank, Request name is invalid"#,
    r#"pattern: ActiveModel::ValidationError: Validation failed: Request name is invalid"#,
    r#"bang true: true false"#,
    r#"model: true ["Request name is invalid"]"#,
    r#"no context: true"#,
    r#"context: ActiveModel::ValidationError: Validation failed: Seat can't be blank"#,
    r#"seated: true"#,
    r#"validate context: false ["Seat can't be blank"]"#,
    r#"validate seated: true"#,
    r#"validate nil: true TrueClass"#,
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", "\n"),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/payments/request.rb", REQUEST),
];

const MORE: [(&str, &str); 2] = [("app/models/ticket.rb", TICKET), ("app/models/vb_probe.rb", PROBE)];

const LOADS: [&str; 4] =
    ["app/models/application_record.rb", "app/models/payments/request.rb", "app/models/ticket.rb", "app/models/vb_probe.rb"];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().chain(MORE.iter()).fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The lowered class defines `validate!` over its own `valid?`.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_tableless_model_defines_validate_bang() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let request = std::fs::read_to_string(emitted.join("app/models/payments/request.rb")).unwrap();
    assert!(request.contains("def validate!"), "{request}");
    assert!(request.contains("valid? || raise(ActiveModel::ValidationError.new(self))"), "{request}");
    let ticket = std::fs::read_to_string(emitted.join("app/models/ticket.rb")).unwrap();
    assert!(ticket.contains("def validate!(context = nil)"), "{ticket}");
    assert!(ticket.contains("valid?(context) || raise("), "{ticket}");
    // An explicit context goes to `valid?`; no argument stays the checks.
    assert!(ticket.contains("def validate(*context)"), "{ticket}");
    assert!(ticket.contains("valid?(context.first)"), "{ticket}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn validate_bang_raises_like_activemodel_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let native_files: Vec<(&str, &str)> = FILES.iter().chain(MORE.iter()).copied().collect();
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "active-model-validate-bang",
        &native_files,
        &emitted,
        "rh_spec_active_model_validate_bang",
        "VbProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
