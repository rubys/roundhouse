//! Baseline-compatible regression for the PostgreSQL text-cast change.
//!
//! This source intentionally uses only APIs that exist at the parent baseline.
//! Copy it into `tests/` in a baseline checkout and run it there: the exact
//! PostgreSQL DDL assertion must fail because the baseline rejects the cast.
//! The same test passes on the feature worktree.

use roundhouse::Symbol;
use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::ingest_schema;

/// Uses only baseline APIs to assert the exact PostgreSQL DDL line that the text-cast feature adds.
#[test]
fn postgres_ddl_accepts_a_mutated_text_cast_with_exact_sql() {
    let mut schema = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "display_name", type: :string, as: "first_name || ''", stored: true
  end
end
"#,
        "db/schema.rb",
    )
    .expect("portable generated expression should ingest");

    let people = schema
        .tables
        .get_mut(&Symbol::from("people"))
        .expect("people table");
    let display_name = people
        .columns
        .iter_mut()
        .find(|column| column.name.as_str() == "display_name")
        .expect("display_name column");
    display_name
        .generated
        .as_mut()
        .expect("generated metadata")
        .expression = "first_name::text".into();

    let statements = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("the bounded PostgreSQL text cast should render");
    assert_eq!(
        statements,
        vec![
            r#"CREATE TABLE IF NOT EXISTS "people" (
  "id" bigserial PRIMARY KEY,
  "first_name" character varying,
  "display_name" character varying GENERATED ALWAYS AS (first_name::text) STORED
)"#
                .to_string()
        ]
    );
}
