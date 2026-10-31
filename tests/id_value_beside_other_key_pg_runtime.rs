//! PostgreSQL: id value beside other key, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE routes (
    id bigint NOT NULL,
    host character varying(255) NOT NULL,
    CONSTRAINT routes_pkey PRIMARY KEY (host)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_id_value_other_key
"#;

const ROUTE: &str = r#"class Route < ApplicationRecord
  before_create :generate_id

  def generate_id
    self.id_value = 4242 unless id_value
  end
end
"#;

const PROBE: &str = r##"class IdValueProbe
  def self.lines
    out = []
    r = Route.new(host: "a.example")
    out << "new: #{r.id.inspect} #{r.id_value.inspect}"
    r.save!
    out << "created: #{r.id.inspect} #{r.id_value.inspect} #{r.id?} #{r.id_value?}"
    f = Route.find("a.example")
    out << "found: #{f.id.inspect} #{f.id_value.inspect}"
    given = Route.create!(host: "b.example", id_value: 7)
    out << "given: #{given.id.inspect} #{given.id_value.inspect} #{Route.where(host: "b.example").pick(:id)}"
    out
  end
end
"##;

const EXPECTED: [&str; 4] = [
    r#"new: "a.example" nil"#,
    r#"created: "a.example" 4242 true true"#,
    r#"found: "a.example" 4242"#,
    r#"given: "b.example" 7 7"#,
];

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/route", "app/models/id_value_probe"];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/route.rb", ROUTE),
    ("app/models/id_value_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn id_value_reads_the_id_column_beside_another_key_like_native_rails() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "id-value-other-key",
        &FILES,
        &emitted,
        "rh_spec_id_value_other_key",
        "IdValueProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// Without a database: the emitted `id_value` reads the `id` column's slot,
/// not the key's, and hydration's `_generated_id=` writes the column, not the
/// key (the module defines `id=` twice; the alias follows the first).
#[test]
#[ignore = "fork spec: postgresql"]
fn id_value_does_not_delegate_to_the_key_reader() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let model = std::fs::read_to_string(emitted.join("app/models/route.rb")).expect("emitted model");
    assert!(model.contains("def id_value\n      @id\n"), "{model}");
    assert!(!model.contains("def id_value\n      self.id\n"), "{model}");
    let alias = model.find("alias_method :_generated_id=, :id=").expect("the hydration alias");
    let key_writer = model.rfind("def id=(value)\n      @host").expect("the key's writer");
    assert!(alias < key_writer, "the alias names the column's writer:\n{model}");
}
