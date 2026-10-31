//! PostgreSQL: column ivar collision, on the Ruby target against native Rails.
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
    code character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_column_ivar_collision
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const CODE_WRAPPER: &str = r##"class CodeWrapper
  attr_reader :text

  def initialize(text)
    @text = text
  end

  def shout
    text.to_s.upcase
  end
end
"##;

const SHOP: &str = r##"class Shop < ApplicationRecord
  def code
    @code ||= CodeWrapper.new(super)
  end

  def code=(value)
    @code = nil
    super(value.try(:text) || value.to_s.strip)
  end
end
"##;

const PROBE: &str = r##"class ColumnIvarProbe
  def self.lines
    out = []
    s = Shop.new(code: " ab ", name: "n")
    out << "memo: #{s.code.class} #{s.code.shout} #{s.code.equal?(s.code)}"
    s.code = "cd"
    out << "rewrite: #{s.code.shout} #{s.read_attribute(:code)} #{s.attributes["code"]}"
    s.save!
    t = Shop.find(s.id)
    out << "loaded: #{t.code.class} #{t.code.shout} #{t.name} #{t.changed?}"
    out << "where: #{Shop.where(code: "cd").count}"
    t.code = CodeWrapper.new("ef")
    t.save!
    out << "wrapper written: #{Shop.find(t.id).read_attribute(:code)}"
    out
  end
end
"##;

const NATIVE_BOOT: &str = r#"railties = $LOAD_PATH.grep(%r{/activerecord/lib\z}).first.sub(%r{/activerecord/lib\z}, "/railties/lib")
$LOAD_PATH.unshift(railties)
require "rails/railtie"
"#;

const EXPECTED: [&str; 5] = [
    "memo: CodeWrapper AB true",
    "rewrite: CD cd cd",
    "loaded: CodeWrapper CD n false",
    "where: 1",
    "wrapper written: ef",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("Gemfile", "source \"https://rubygems.org\"\n"),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/code_wrapper.rb", CODE_WRAPPER),
    ("app/models/shop.rb", SHOP),
    ("app/models/column_ivar_probe.rb", PROBE),
    ("native_boot.rb", NATIVE_BOOT),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn a_models_own_column_named_ivar_is_not_the_column_storage_as_natively_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "column-ivar-collision",
        &FILES,
        &emitted,
        "rh_spec_column_ivar_collision",
        "ColumnIvarProbe",
        &[
            "native_boot",
            "app/models/application_record",
            "app/models/code_wrapper",
            "app/models/shop",
            "app/models/column_ivar_probe",
        ],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// Only the column the authored code names moves; the model's own ivar
/// stays the model's, and an untouched column keeps its plain storage.
#[test]
#[ignore = "fork spec: postgresql"]
fn only_a_column_authored_code_names_gets_its_own_storage_ivar() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shop = std::fs::read_to_string(emitted.join("app/models/shop.rb")).expect("shop.rb");
    let include = shop.find("  include GeneratedAttributeMethods\n").expect(&shop);
    let (module, class) = shop.split_at(include);
    assert!(module.contains("@__attr_code"), "{shop}");
    assert!(!module.contains("@code"), "{shop}");
    assert!(class.contains("@code ||= CodeWrapper.new(super)"), "{shop}");
    assert!(class.contains("@code = nil"), "{shop}");
    assert!(module.contains("@name"), "{shop}");
    assert!(!shop.contains("@__attr_name"), "{shop}");
}
