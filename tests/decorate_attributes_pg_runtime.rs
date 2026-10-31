//! PostgreSQL: decorate attributes, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE baskets (
    id bigserial NOT NULL,
    title character varying,
    items text,
    CONSTRAINT baskets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_decorate_attributes
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const BASKET: &str = r#"class Basket < ApplicationRecord
  alias_attribute :label, :title
  coder = PipeCoder
  decorate_attributes([:items]) do |name, type|
    TrimDecorator.new(name, coder, type)
  end
  serialize :items, coder: coder
end
"#;

/// A coder class: it answers `load`, so Rails uses the class itself.
const PIPE_CODER: &str = r#"class PipeCoder
  def self.load(payload)
    payload.nil? ? [] : payload.split("|")
  end

  def self.dump(value)
    value.nil? ? nil : value.join("|")
  end
end
"#;

const TRIM_DECORATOR: &str = r#"class TrimDecorator
  attr_reader :name

  def initialize(name, coder, decorated_type)
    @name = name
    @coder = coder
    @decorated_type = decorated_type
  end

  def deserialize(value)
    value.nil? ? nil : value.split("|").map(&:strip).join("|")
  end

  def changed_in_place?(raw_old_value, value)
    raw_old_value = @coder.dump(@coder.load(deserialize(raw_old_value))) if raw_old_value.is_a?(String)
    @decorated_type.changed_in_place?(raw_old_value, value)
  end

  def method_missing(method, *args, &block)
    if @decorated_type.respond_to?(method)
      @decorated_type.public_send(method, *args, &block)
    else
      super
    end
  end

  def respond_to_missing?(name, include_private = false)
    @decorated_type.respond_to?(name) || super
  end
end
"#;

const PROBE: &str = r##"class DecorateAttributesProbe
  def self.lines
    out = []
    a = Basket.create!(title: "a", items: ["x", "y"])
    Basket.connection.execute("UPDATE baskets SET items = ' p | q ' WHERE id = #{a.id}")
    found = Basket.find(a.id)
    out << "read: #{found.items.inspect}"
    out << "changed: #{found.changed?}"
    out << "before_type_cast: #{found.read_attribute_before_type_cast(:title).inspect} #{found.read_attribute_before_type_cast("label").inspect} #{found.read_attribute_before_type_cast(:nope).inspect}"
    type = Basket.type_for_attribute(:items)
    out << "types: #{type.class} #{type.subtype.class} #{type.subtype.name.inspect} #{type.type.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 4] = [
    r#"read: ["p", "q"]"#,
    "changed: false",
    r#"before_type_cast: "a" "a" nil"#,
    r#"types: ActiveRecord::Type::Serialized TrimDecorator "items" :text"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/pipe_coder",
    "app/models/trim_decorator",
    "app/models/application_record",
    "app/models/basket",
    "app/models/decorate_attributes_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/basket.rb", BASKET),
    ("app/models/pipe_coder.rb", PIPE_CODER),
    ("app/models/trim_decorator.rb", TRIM_DECORATOR),
    ("app/models/decorate_attributes_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the column consults the declared type, and the
/// model carries the `decorate_attributes` call, before `serialize`.
#[test]
#[ignore = "fork spec: postgresql"]
fn decorate_attributes_runs_at_class_load_before_serialize() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let basket = std::fs::read_to_string(emitted.join("app/models/basket.rb")).unwrap();
    assert!(basket.contains(r#"self.class._attribute_deserialize("items", value)"#), "{basket}");
    let decorate = basket.find("decorate_attributes([:items])").unwrap_or_else(|| panic!("no decorate_attributes in:\n{basket}"));
    let serialize = basket.find("serialize :items, coder: coder").unwrap_or_else(|| panic!("no serialize in:\n{basket}"));
    assert!(decorate < serialize, "{basket}");
    assert!(basket.contains("TrimDecorator.new(name, coder, type)"), "{basket}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn decorate_attributes_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "decorate-attributes",
        &FILES,
        &emitted,
        "rh_spec_decorate_attributes",
        "DecorateAttributesProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
