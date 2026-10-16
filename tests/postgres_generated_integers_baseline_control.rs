//! Baseline-compatible negative control for PostgreSQL generated int4 casts.
//!
//! Copy this file unchanged into `tests/` in the pristine parent baseline. It
//! uses only APIs and enum variants that already exist there. The exact DDL
//! assertion is expected to fail before this feature and pass on the feature
//! worktree.

use roundhouse::Symbol;
use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::ingest_schema;
use roundhouse::schema::ColumnType;

/// Uses baseline APIs to assert the exact DDL added for a text-to-int4 generated result.
#[test]
fn postgres_ddl_renders_json_extraction_cast_to_exact_int4() {
    let mut schema = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "counters", force: :cascade do |t|
    t.json "payload_json"
    t.string "first_name"
    t.virtual "counter", type: :string, as: "first_name || ''", stored: true
  end
end
"#,
        "db/schema.rb",
    )
    .expect("portable source schema should ingest before replacing its expression");

    let counters = schema
        .tables
        .get_mut(&Symbol::from("counters"))
        .expect("counters table");
    let counter = counters
        .columns
        .iter_mut()
        .find(|column| column.name.as_str() == "counter")
        .expect("counter generated column");
    counter.col_type = ColumnType::Integer;
    counter
        .generated
        .as_mut()
        .expect("generated metadata")
        .expression = "(payload_json ->> 'counter'::text)::integer".into();

    let statements = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("bounded PostgreSQL text-to-int4 generated result should render");
    assert_eq!(
        statements,
        vec![
            r#"CREATE TABLE IF NOT EXISTS "counters" (
  "id" bigserial PRIMARY KEY,
  "payload_json" json,
  "first_name" character varying,
  "counter" integer GENERATED ALWAYS AS ((payload_json ->> 'counter'::text)::integer) STORED
)"#
                .to_string()
        ]
    );
}
