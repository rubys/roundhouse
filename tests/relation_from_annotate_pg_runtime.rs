//! PostgreSQL: relation from annotate, on the Ruby target against native Rails.
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
  database: rh_spec_relation_from_annotate
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  def self.annotate(&block)
    block.call(self) if block
    :documented
  end
end
"#;

const PROBE: &str = r##"class FromAnnotateProbe
  def self.lines
    out = []
    Item.create!(kind: "a", name: "one")
    Item.create!(kind: "a", name: "two")
    Item.create!(kind: "b", name: "three")
    named = Item.from(Item.where(kind: "a"), :items).order(:name).map(&:name)
    out << "from named: #{named.inspect}"
    out << "from default: #{Item.from(Item.where(kind: "a").select("name"), :sub).select("sub.name").order("sub.name").map(&:name).inspect}"
    out << "from string: #{Item.from("items").where(kind: "b").map(&:name).inspect}"
    out << "from count: #{Item.from(Item.where(kind: "a"), :items).count}"
    commented = Item.where(kind: "a").annotate("loaded by probe").annotate("second")
    out << "annotate sql: #{commented.to_sql.include?("/* loaded by probe */ /* second */")} #{commented.to_sql.end_with?("*/")}"
    out << "annotate rows: #{commented.order(:name).map(&:name).inspect}"
    out << "annotate escapes: #{Item.all.annotate("a */ b").to_sql.include?("/* a * / b */")}"
    out << "class annotate: #{Item.annotate { }.inspect}"
    conn = Item.connection
    out << "exec_query: #{conn.exec_query("SELECT name FROM items WHERE kind = 'b'", "Probe Load").rows.inspect}"
    out << "select_all: #{conn.select_all("SELECT name FROM items WHERE kind = 'a' ORDER BY name", "Probe Load").rows.inspect}"
    out << "select_all relation: #{conn.select_all(Item.where(kind: "b").select(:name), "Probe Load").rows.inspect}"
    out << "binds: #{conn.exec_query("SELECT name FROM items WHERE kind = $1 ORDER BY name", "Probe", ["a"]).rows.inspect}"
    out
  end
end
"##;


const EXPECTED: [&str; 12] = [
    r#"from named: ["one", "two"]"#,
    r#"from default: ["one", "two"]"#,
    r#"from string: ["three"]"#,
    "from count: 2",
    "annotate sql: true true",
    r#"annotate rows: ["one", "two"]"#,
    "annotate escapes: true",
    "class annotate: :documented",
    r#"exec_query: [["three"]]"#,
    r#"select_all: [["one"], ["two"]]"#,
    r#"select_all relation: [["three"]]"#,
    r#"binds: [["one"], ["two"]]"#,
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/item.rb", ITEM),
    ("app/models/from_annotate_probe.rb", PROBE),
];

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/item", "app/models/from_annotate_probe"];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The models natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn relation_from_annotate_and_raw_queries_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation-from-annotate",
        &FILES,
        &emitted,
        "rh_spec_relation_from_annotate",
        "FromAnnotateProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
