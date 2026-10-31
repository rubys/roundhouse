//! PostgreSQL: compiler refusals, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE gadgets (
    id bigserial NOT NULL,
    name character varying,
    updated_at timestamp(6) without time zone,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);
CREATE TABLE parts (
    id bigserial NOT NULL,
    gadget_id bigint,
    name character varying,
    updated_at timestamp(6) without time zone,
    CONSTRAINT parts_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_refusals
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  def self.constraint_options(column)
    return {} if column == :none

    { foreign_key: :gadget_id, inverse_of: :gadget }
  end
end
"#;

const GADGET: &str = r#"class Gadget < ApplicationRecord
  has_many :parts,
    class_name: "Part",
    dependent: :destroy,
    **constraint_options(:shop_id)

  has_many :plain_parts, class_name: "Part", foreign_key: :gadget_id, **constraint_options(:none)
end
"#;

const PART: &str = r#"class Part < ApplicationRecord
  belongs_to :gadget, optional: true
end
"#;

const NOTIFIER: &str = r#"class Notifier
  def self.should_notify?(owner)
    return false if ActiveRecord::NoTouching.applied_to?(owner.class)

    true
  end

  #: (untyped) -> untyped
  def self.parse_datetime(value)
    value = value.try(:to_time)
    value if value.acts_like?(:time)
  end

  #: (untyped) -> untyped
  def self.date_like(value)
    value.nil? || value.acts_like?(:date) ? :as_is : :parse
  end
end
"#;

const PROBE: &str = r##"class RefusalsProbe
  def self.lines
    out = []
    gadget = Gadget.create!(name: "g")
    part = Part.create!(name: "p", gadget_id: gadget.id)

    out << "applied outside: #{ActiveRecord::NoTouching.applied_to?(Gadget)}"
    out << "notify outside: #{Notifier.should_notify?(gadget)}"
    Gadget.no_touching do
      out << "applied inside: #{ActiveRecord::NoTouching.applied_to?(Gadget)} #{ActiveRecord::NoTouching.applied_to?(Part)}"
      out << "notify inside: #{Notifier.should_notify?(gadget)} #{Notifier.should_notify?(part)}"
      before = Gadget.find(gadget.id).updated_at
      gadget.touch
      out << "touch inside: #{gadget.no_touching?} #{Gadget.find(gadget.id).updated_at == before}"
    end
    gadget.touch
    out << "touch after: #{gadget.no_touching?} #{!Gadget.find(gadget.id).updated_at.nil?}"
    out << "applied after: #{ActiveRecord::NoTouching.applied_to?(Gadget)}"
    begin
      Gadget.no_touching { raise "boom" }
    rescue RuntimeError
      out << "stack after raise: #{ActiveRecord::NoTouching.applied_to?(Gadget)}"
    end

    out << "acts_like: #{Notifier.parse_datetime("2026-01-02 03:04:05 UTC").class} #{Notifier.parse_datetime(nil).inspect} #{Notifier.date_like(Date.new(2026, 1, 2))} #{Notifier.date_like("x")}"

    options = Gadget.reflect_on_association(:parts).options
    out << "splat options: #{options.sort_by { |k, _| k.to_s }.inspect}"
    out << "empty splat: #{Gadget.reflect_on_association(:plain_parts).options.sort_by { |k, _| k.to_s }.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 11] = [
    "applied outside: false",
    "notify outside: true",
    "applied inside: true false",
    "notify inside: false true",
    "touch inside: true true",
    "touch after: false true",
    "applied after: false",
    "stack after raise: false",
    "acts_like: Time nil as_is parse",
    r#"splat options: [[:class_name, "Part"], [:dependent, :destroy], [:foreign_key, :gadget_id], [:inverse_of, :gadget]]"#,
    r#"empty splat: [[:class_name, "Part"], [:foreign_key, :gadget_id]]"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/gadget",
    "app/models/part",
    "app/models/notifier",
    "app/models/refusals_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/gadget.rb", GADGET),
    ("app/models/part.rb", PART),
    ("app/models/notifier.rb", NOTIFIER),
    ("app/models/refusals_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: no site is written as a raise, `acts_like?` and the
/// constant stay calls, and the splat is evaluated where the reflection is built.
#[test]
#[ignore = "fork spec: postgresql"]
fn refused_constructs_are_carried_as_written() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let notifier = std::fs::read_to_string(emitted.join("app/models/notifier.rb")).unwrap();
    assert!(!notifier.contains("not supported"), "{notifier}");
    assert!(notifier.contains("ActiveRecord::NoTouching.applied_to?(owner.class)"), "{notifier}");
    assert!(notifier.contains("value.acts_like?(:time)"), "{notifier}");
    let gadget = std::fs::read_to_string(emitted.join("app/models/gadget.rb")).unwrap();
    assert!(gadget.contains(".merge(constraint_options(:shop_id))"), "{gadget}");
    assert!(!gadget.contains("unsupported:"), "{gadget}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn refused_constructs_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "refusals-refusals",
        &FILES,
        &emitted,
        "rh_spec_refusals",
        "RefusalsProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
