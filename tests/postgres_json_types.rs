//! PostgreSQL `json` and `jsonb` carry distinct source semantics even
//! though both use the shared runtime's serialized-text JSON boundary.

use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::structure_sql::ingest_structure_sql;
use roundhouse::ingest::{ingest_migration, ingest_schema};
use roundhouse::schema::Schema;
use roundhouse::Symbol;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SCHEMA_RB: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.json "payload_json"
    t.jsonb "payload_jsonb"
  end
end
"#;

const STRUCTURE_SQL: &str = r#"CREATE TABLE public.documents (
    id bigint NOT NULL,
    payload_json json,
    payload_jsonb jsonb
);

ALTER TABLE ONLY public.documents
    ADD CONSTRAINT documents_pkey PRIMARY KEY (id);
"#;

const GENERATED_DDL_START: &str = "BEGIN_JSON_TYPES_POSTGRES_DDL";
const GENERATED_DDL_END: &str = "END_JSON_TYPES_POSTGRES_DDL";

/// Build the generic two-column source fixture through the public schema ingester.
fn schema_from_rb() -> Schema {
    ingest_schema(SCHEMA_RB.as_bytes(), "db/schema.rb").expect("schema.rb should ingest")
}

/// Read the serialized column kind so this regression also compiles on the old enum.
fn column_kind(schema: &Schema, column_name: &str) -> String {
    let table = schema.tables.get(&Symbol::from("documents")).expect("documents table");
    let column = table
        .columns
        .iter()
        .find(|column| column.name.as_str() == column_name)
        .unwrap_or_else(|| panic!("missing column {column_name}"));
    serde_json::to_value(&column.col_type)
        .expect("column type serializes")
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("column type has no kind: {:?}", column.col_type))
        .to_owned()
}

/// Check both source declarations retain their expected serialized type tags.
fn assert_json_kinds(schema: &Schema, expected_json: &str, expected_jsonb: &str) {
    assert_eq!(column_kind(schema, "payload_json"), expected_json);
    assert_eq!(column_kind(schema, "payload_jsonb"), expected_jsonb);
}

/// Match one emitted column type without confusing json with the jsonb prefix.
fn postgres_has_column_type(statement: &str, name: &str, expected_type: &str) -> bool {
    let prefix = format!("\"{name}\" ");
    statement.lines().any(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split_whitespace().next())
            .is_some_and(|type_name| type_name.trim_end_matches(',') == expected_type)
    })
}

/// Keep source type fidelity across schema.rb ingestion and schema serialization.
#[test]
fn schema_rb_and_serde_preserve_json_and_jsonb_as_distinct_types() {
    let schema = schema_from_rb();
    assert_json_kinds(&schema, "json", "jsonb");

    let serialized = serde_json::to_value(&schema).expect("schema serializes");
    let round_tripped: Schema = serde_json::from_value(serialized).expect("schema deserializes");
    assert_eq!(round_tripped, schema);
    assert_json_kinds(&round_tripped, "json", "jsonb");
}

/// Preserve the two PostgreSQL types when ingesting a structure dump.
#[test]
fn structure_sql_preserves_json_and_jsonb_source_types() {
    let schema = ingest_structure_sql(STRUCTURE_SQL.as_bytes(), "db/structure.sql")
        .expect("structure.sql should ingest");
    assert_json_kinds(&schema, "json", "jsonb");
}

/// Retain each declared JSON type while folding add-column and change-column migrations.
#[test]
fn migration_add_and_change_column_keep_json_and_jsonb_distinct() {
    let mut schema = schema_from_rb();
    let migration = r#"class ChangeDocuments < ActiveRecord::Migration[8.1]
  def change
    add_column :documents, :added_json, :json
    add_column :documents, :added_jsonb, :jsonb
    change_column :documents, :payload_json, :jsonb
    change_column :documents, :payload_jsonb, :json
  end
end
"#;
    ingest_migration(migration.as_bytes(), "db/migrate/change_documents.rb", &mut schema)
        .expect("JSON column migration should fold");

    assert_eq!(column_kind(&schema, "payload_json"), "jsonb");
    assert_eq!(column_kind(&schema, "payload_jsonb"), "json");
    assert_eq!(column_kind(&schema, "added_json"), "json");
    assert_eq!(column_kind(&schema, "added_jsonb"), "jsonb");
}

/// Emit distinct PostgreSQL types while preserving the shared SQLite storage contract.
#[test]
fn postgres_ddl_distinguishes_json_and_jsonb_while_sqlite_keeps_text_storage() {
    let schema = schema_from_rb();
    let postgres = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("PostgreSQL DDL should render");
    let postgres_table = postgres
        .iter()
        .find(|statement| statement.contains("CREATE TABLE") && statement.contains("documents"))
        .expect("documents CREATE TABLE");

    // The markers let the campaign harness capture the actual generated
    // DDL and apply it to PostgreSQL before running the SQL oracle.
    println!("{GENERATED_DDL_START}");
    for statement in &postgres {
        println!("{statement};");
    }
    println!("{GENERATED_DDL_END}");

    assert!(
        postgres_has_column_type(postgres_table, "payload_json", "json"),
        "{postgres_table}"
    );
    assert!(
        postgres_has_column_type(postgres_table, "payload_jsonb", "jsonb"),
        "{postgres_table}"
    );

    let sqlite = render_schema_statements_for(&schema, Dialect::Sqlite)
        .expect("SQLite DDL should render");
    assert_eq!(
        sqlite,
        vec![
            "CREATE TABLE IF NOT EXISTS documents (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  payload_json TEXT,\n  payload_jsonb TEXT\n)"
        ],
        "both source types retain the existing SQLite text-backed representation"
    );
}

/// Execute emitted CRuby create, read and update flows through both JSON accessors.
#[test]
fn json_and_jsonb_runtime_accessors_still_use_the_shared_text_json_boundary() {
    let run = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.json \"payload_json\"\n    t.jsonb \"payload_jsonb\"",
        )
        .write(
            "test/models/article_json_types_test.rb",
            r#"require "test_helper"

class ArticleJsonTypesTest < ActiveSupport::TestCase
  test "json and jsonb use the same decoded public accessors" do
    value = { "nested" => [1, "two", true] }
    article = Article.create!(
      title: "JSON types",
      body: "A body long enough to satisfy validation.",
      payload_json: value,
      payload_jsonb: value
    )

    assert_equal value, article.payload_json
    assert_equal value, article.payload_jsonb
    assert_equal value, article[:payload_json]
    assert_equal value, article[:payload_jsonb]

    article.payload_json = { "changed" => 3 }
    article.payload_jsonb = { "changed" => 4 }
    article.save!
    reloaded = Article.find(article.id)
    assert_equal({ "changed" => 3 }, reloaded.payload_json)
    assert_equal({ "changed" => 4 }, reloaded.payload_jsonb)
  end
end
"#,
        )
        .run_test("test/models/article_json_types_test.rb");

    run.assert_passes();
}
