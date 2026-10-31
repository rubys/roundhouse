//! PostgreSQL: sti lookup table, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE contract_configs (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    CONSTRAINT contract_configs_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sti_lookup_table
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
    not_all_strings = mapping.any? { |_key, value| !value.is_a?(String) }
    raise ArgumentError, "constants should map only to a single string value" if not_all_strings

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

const CONTRACT_CONFIG: &str = r#"module Pay
  class ContractConfig < ApplicationRecord
    self.table_name = "contract_configs"

    extend StiLookupTable

    self.sti_mapping = {
      "Pay::Entities::AppConfig" => "AppConfig",
      "Pay::OtherConfig" => "OtherConfig"
    }
  end
end
"#;

const APP_CONFIG: &str = r#"module Pay
  module Entities
    class AppConfig < Pay::ContractConfig
    end
  end
end
"#;

const OTHER_CONFIG: &str = r#"module Pay
  class OtherConfig < ContractConfig
  end
end
"#;

const PROBE: &str = r##"class StiLookupTableProbe
  def self.lines
    out = []
    Pay::Entities::AppConfig.create!(name: "a")
    Pay::OtherConfig.create!(name: "o")
    out << "types: #{Pay::ContractConfig.order(:id).pluck(:type).inspect}"
    out << "base: #{Pay::ContractConfig.order(:id).map { |c| c.class.name }.join(",")}"
    out << "sub: #{Pay::Entities::AppConfig.count} #{Pay::OtherConfig.all.map(&:name).join(",")}"
    a = Pay::Entities::AppConfig.first
    out << "find: #{Pay::ContractConfig.find(a.id).class.name} #{Pay::Entities::AppConfig.find(a.id).name}"
    out << "names: #{Pay::Entities::AppConfig.sti_name} #{Pay::ContractConfig.sti_class_for("OtherConfig").name}"
    begin
      Pay::ContractConfig.sti_class_for("Nope")
      out << "unmapped: found"
    rescue StiLookupTable::SubclassNotFound => e
      out << "unmapped: #{e.message}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 6] = [
    "types: [\"AppConfig\", \"OtherConfig\"]",
    "base: Pay::Entities::AppConfig,Pay::OtherConfig",
    "sub: 1 o",
    "find: Pay::Entities::AppConfig a",
    "names: AppConfig Pay::OtherConfig",
    "unmapped: Nope is not mapped to a known subclass in Pay::ContractConfig.sti_mapping",
];

const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/sti_lookup_table",
    "app/models/pay/contract_config",
    "app/models/pay/entities/app_config",
    "app/models/pay/other_config",
    "app/models/sti_lookup_table_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/sti_lookup_table.rb", STI_LOOKUP_TABLE),
    ("app/models/pay/contract_config.rb", CONTRACT_CONFIG),
    ("app/models/pay/entities/app_config.rb", APP_CONFIG),
    ("app/models/pay/other_config.rb", OTHER_CONFIG),
    ("app/models/sti_lookup_table_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The base keeps the `extend` and its mapping, and hydrates through
/// the runtime `find_sti_class`.
#[test]
#[ignore = "fork spec: postgresql"]
fn sti_lookup_table_base_keeps_its_extend_and_mapping() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let base = std::fs::read_to_string(emitted.join("app/models/pay/contract_config.rb")).unwrap();
    let extend = base.find("extend StiLookupTable").unwrap_or_else(|| panic!("{base}"));
    let mapping = base.find("self.sti_mapping = {").unwrap_or_else(|| panic!("{base}"));
    // The `extended` hook resets the mapping, so the extend runs first.
    assert!(extend < mapping, "{base}");
    assert!(base.contains("find_sti_class(row.type"), "{base}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn sti_lookup_table_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-lookup-table",
        &FILES,
        &emitted,
        "rh_spec_sti_lookup_table",
        "StiLookupTableProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
