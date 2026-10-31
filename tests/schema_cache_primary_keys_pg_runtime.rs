//! PostgreSQL: schema cache primary keys, on the Ruby target against native Rails.
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
    label character varying,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);

CREATE TABLE gadget_parts (
    part_id integer NOT NULL,
    gadget_id integer NOT NULL,
    note text,
    CONSTRAINT gadget_parts_pkey PRIMARY KEY (gadget_id, part_id)
);

CREATE TABLE gadget_logs (
    entry text
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_schema_cache_primary_keys
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const GADGET: &str = "class Gadget < ApplicationRecord\nend\n";

const PROBE: &str = r##"class SchemaCachePrimaryKeysProbe
  def self.lines
    out = []
    connection = Gadget.connection
    cache = connection.schema_cache
    %w[gadgets gadget_parts gadget_logs no_such_table].each do |table|
      out << "#{table}: #{cache.primary_keys(table).inspect} #{cache.primary_keys(table).equal?(cache.primary_keys(table))} #{cache.primary_keys(table)&.frozen?.inspect}"
    end
    %w[gadgets gadget_parts].each do |table|
      out << "connection #{table}: #{connection.primary_key(table).inspect} #{connection.primary_keys(table).inspect}"
    end
    out << "array: #{connection.primary_keys(%w[gadgets gadget_parts]).inspect}"
    out << "no key: #{connection.primary_key("gadget_logs").inspect}"
    out
  end
end
"##;

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/gadget.rb", GADGET),
    ("app/models/schema_cache_primary_keys_probe.rb", PROBE),
];

/// What native Rails requires, in order.
const LOADS: [&str; 3] = [
    "app/models/application_record",
    "app/models/gadget",
    "app/models/schema_cache_primary_keys_probe",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_schema_cache_answers_primary_keys_from_the_catalog() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "schema-cache-primary-keys",
        &FILES,
        &emitted,
        "rh_spec_schema_cache_primary_keys",
        "SchemaCachePrimaryKeysProbe",
        &LOADS,
    ) else { return };
    assert!(native[0].starts_with(r#"gadgets: "id" "#), "{native:#?}");
    assert!(native[1].starts_with(r#"gadget_parts: ["gadget_id", "part_id"] "#), "{native:#?}");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
