//! `create_virtual_table "message_search_index", "fts5", [...]`.
//!
//! A table the DB builds from a MODULE rather than from a column list.
//! It was not ingested at all, so it never reached the DDL — and
//! campfire's `Message::Searchable` writes to it by hand on every
//! message commit, so once those callbacks started firing every insert
//! died on "no such table: message_search_index".
//!
//! It is a real table with no model, which is also why the test
//! harness has to clear it between tests: reloading fixtures on top of
//! rows nothing removed is the same duplicate-row bug the per-model
//! truncate list exists to prevent.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::shared::schema_sql::render_schema_statements;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn statements() -> Vec<String> {
    let app = ingest_app_from_tree(tree(&[(
        "db/schema.rb",
        r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body", null: false
  end
  create_virtual_table "message_search_index", "fts5", ["body", "tokenize=porter"]
end
"#,
    )]))
    .expect("ingest virtual-table schema");
    render_schema_statements(&app.schema)
}

/// The module and its argument list are rendered back verbatim — fts5
/// mixes column names and `tokenize=…` options in one list, so parsing
/// them finer would be inventing a grammar per module.
#[test]
fn a_virtual_table_renders_with_its_module() {
    let stmts = statements();
    assert!(
        stmts.iter().any(|s| s
            == "CREATE VIRTUAL TABLE IF NOT EXISTS message_search_index USING fts5(body, tokenize=porter)"),
        "got {stmts:?}"
    );
}

/// Never through the column-list branch: no types, no NOT NULL, and no
/// AUTOINCREMENT key — sqlite rejects all three on a virtual table.
#[test]
fn it_never_renders_as_a_plain_create_table() {
    let stmts = statements();
    assert!(
        !stmts.iter().any(|s| s.contains("CREATE TABLE IF NOT EXISTS message_search_index")),
        "got {stmts:?}"
    );
    assert!(
        stmts.iter().any(|s| s.contains("CREATE TABLE IF NOT EXISTS messages")),
        "the ordinary table beside it is unaffected: {stmts:?}"
    );
}
