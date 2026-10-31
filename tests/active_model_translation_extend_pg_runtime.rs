//! PostgreSQL: active model translation extend, on the Ruby target against native Rails.
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
  database: rh_spec_active_model_translation_extend
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const COLLECTOR: &str = r#"class OutputCollector
  extend ActiveModel::Naming
  extend ActiveModel::Translation

  attr_accessor :message, :errors

  def initialize
    @errors = ActiveModel::Errors.new(self)
  end

  def unrunnable
    @message = errors.full_message(:export, "Failed export")
  end

  def fail!(operation, text)
    errors.add(operation, text)
  end
end
"#;

const CSV: &str = r#"class CsvCollector < OutputCollector
end
"#;

const PROBE: &str = r##"class TeProbe
  def self.lines
    out = []
    c = CsvCollector.new
    c.fail!(:export, "Failed export")
    out << "full: #{c.unrunnable}"
    out << "human: #{CsvCollector.human_attribute_name(:export_name)} #{OutputCollector.human_attribute_name(:id)}"
    out << "model_name: #{CsvCollector.model_name.human}"
    out
  end
end
"##;

const EXPECTED: [&str; 3] = [
    "full: Export Failed export",
    "human: Export name Id",
    "model_name: Csv collector",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", "\n"),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/utils/output_collector.rb", COLLECTOR),
    ("app/utils/csv_collector.rb", CSV),
];

const MORE: [(&str, &str); 1] = [("app/models/te_probe.rb", PROBE)];

const LOADS: [&str; 4] = [
    "app/models/application_record.rb",
    "app/utils/output_collector.rb",
    "app/utils/csv_collector.rb",
    "app/models/te_probe.rb",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().chain(MORE.iter()).fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_translation_extend_is_emitted() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let collector = std::fs::read_to_string(emitted.join("app/models/output_collector.rb")).unwrap();
    assert!(collector.contains("extend ActiveModel::Naming"), "{collector}");
    assert!(collector.contains("extend ActiveModel::Translation"), "{collector}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn translation_extend_answers_like_activemodel_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let native_files: Vec<(&str, &str)> = FILES.iter().chain(MORE.iter()).copied().collect();
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "active-model-translation-extend",
        &native_files,
        &emitted,
        "rh_spec_active_model_translation_extend",
        "TeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
