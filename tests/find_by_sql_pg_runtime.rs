//! PostgreSQL: find by sql, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE items (
    id bigserial NOT NULL,
    kind character varying,
    name character varying,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_find_by_sql
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  def self.named_in(kind)
    find_by_sql(["SELECT * FROM items WHERE kind = ? ORDER BY name", kind])
  end
end
"#;

const PROBE: &str = r##"class FindBySqlProbe
  def self.lines
    out = []
    Item.create!(kind: "a", name: "one")
    Item.create!(kind: "a", name: "two")
    Item.create!(kind: "b", name: "three")
    all = Item.find_by_sql("SELECT * FROM items ORDER BY name")
    out << "string: #{all.map(&:name).inspect} #{all.first.class}"
    out << "array: #{Item.named_in("a").map(&:name).inspect}"
    out << "binds: #{Item.find_by_sql("SELECT * FROM items WHERE kind = $1 ORDER BY name", ["b"]).map(&:name).inspect}"
    partial = Item.find_by_sql("SELECT id, name FROM items WHERE kind = 'b'").first
    out << "partial: #{partial.name} #{partial.has_attribute?(:kind)}"
    seen = []
    Item.find_by_sql("SELECT * FROM items WHERE kind = 'a' ORDER BY name") { |r| seen << r.name }
    out << "block: #{seen.inspect}"
    out << "empty: #{Item.find_by_sql("SELECT * FROM items WHERE kind = 'z'").inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 6] = [
    r#"string: ["one", "three", "two"] Item"#,
    r#"array: ["one", "two"]"#,
    r#"binds: ["three"]"#,
    "partial: three false",
    r#"block: ["one", "two"]"#,
    "empty: []",
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/item.rb", ITEM),
    ("app/models/find_by_sql_probe.rb", PROBE),
];

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/item", "app/models/find_by_sql_probe"];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The models natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn find_by_sql_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "find-by-sql",
        &FILES,
        &emitted,
        "rh_spec_find_by_sql",
        "FindBySqlProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
