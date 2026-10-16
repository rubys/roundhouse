//! Baseline-compatible negative control for PostgreSQL JSON extraction.
//!
//! Copy this test unchanged to `tests/` in the pristine baseline worktree and
//! run it there. It uses only pre-existing APIs and no direct `ColumnType`
//! variant. The exact DDL assertion is expected to fail before the JSON
//! extraction feature and pass on the feature worktree.

use roundhouse::Symbol;
use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::ingest_schema;

/// Uses baseline APIs to assert the exact PostgreSQL DDL line added for JSON extraction.
#[test]
fn postgres_ddl_renders_a_mutated_json_extraction_with_exact_sql() {
    let mut schema = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.json "payload_json"
    t.string "first_name"
    t.virtual "display", type: :string, as: "first_name || ''", stored: true
  end
end
"#,
        "db/schema.rb",
    )
    .expect("portable source schema should ingest before replacing the expression");

    let documents = schema
        .tables
        .get_mut(&Symbol::from("documents"))
        .expect("documents table");
    let display = documents
        .columns
        .iter_mut()
        .find(|column| column.name.as_str() == "display")
        .expect("display column");
    display
        .generated
        .as_mut()
        .expect("generated metadata")
        .expression = "payload_json ->> 'model'::text".into();

    let statements = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("bounded PostgreSQL JSON extraction should render");
    assert_eq!(
        statements,
        vec![
            r#"CREATE TABLE IF NOT EXISTS "documents" (
  "id" bigserial PRIMARY KEY,
  "payload_json" json,
  "first_name" character varying,
  "display" character varying GENERATED ALWAYS AS (payload_json ->> 'model'::text) STORED
)"#
                .to_string()
        ]
    );
}
