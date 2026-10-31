//! PostgreSQL: insert all app wrapper, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE tokens (
    id bigint NOT NULL,
    name character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    CONSTRAINT tokens_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_insert_all_wrapper
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const TOKEN: &str = "class Token < ApplicationRecord\nend\n";

const PATCH: &str = r##"module IdAllocatingInsert
  module Wrapper
    def insert_all(records, **options)
      records = allocate(records)
      super
    end

    def insert_all!(records, **options)
      records = allocate(records)
      super
    end

    private

    def allocate(records)
      records.map { |row| row.key?(:id) ? row : row.merge(id: row[:name].ord) }
    end
  end

  ActiveRecord::Relation.prepend(Wrapper)
end
"##;

const PROBE: &str = r##"class InsertAllWrapperProbe
  def self.row(name)
    { name: name, created_at: Time.now, updated_at: Time.now }
  end

  def self.lines
    out = []
    bang = Token.insert_all!([row("a"), row("b")], returning: %w[id])
    out << "insert_all!: #{bang.rows.flatten.inspect}"
    plain = Token.insert_all([row("c"), row("d"), row("e")])
    out << "insert_all: #{plain.affected_rows}"
    out << "ids: #{Token.order(:id).pluck(:id).inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 3] = ["insert_all!: [97, 98]", "insert_all: 3", "ids: [97, 98, 99, 100, 101]"];

const LOADS: [&str; 4] =
    ["app/models/application_record", "app/models/token", "app/models/id_allocating_insert", "app/models/insert_all_wrapper_probe"];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/token.rb", TOKEN),
    ("app/models/id_allocating_insert.rb", PATCH),
    ("app/models/insert_all_wrapper_probe.rb", PROBE),
];

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_app_insert_all_wrapper_runs_around_the_model_call_like_native_rails_on_postgresql() {
    let files = FILES.to_vec();
    let overlay = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "insert-all-wrapper",
        &files,
        &emitted,
        "rh_spec_insert_all_wrapper",
        "InsertAllWrapperProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
