//! `t.uuid` columns and non-default primary keys in `db/schema.rb`
//! (#83).
//!
//! `t.uuid` had no `ColumnType` mapping, so `column_from_call` returned
//! None and the column vanished — while `t.index ["owner_id"]` on it
//! was still emitted, so the DDL failed with `no such column`. On the
//! app that reported it, 15 of 39 tables lost a column that way, with
//! no diagnostic. `create_table …, primary_key: "identifier", id:
//! :string` was likewise ignored: the table got an `id INTEGER PRIMARY
//! KEY AUTOINCREMENT` it never declared.

use roundhouse::emit::shared::schema_sql::render_schema_statements;
use roundhouse::ingest::ingest_schema;

fn ddl(schema_rb: &str) -> Vec<String> {
    let schema = ingest_schema(schema_rb.as_bytes(), "db/schema.rb").expect("ingest schema");
    render_schema_statements(&schema)
}

#[test]
fn uuid_column_is_text_and_its_index_applies() {
    let out = ddl(
        r#"ActiveRecord::Schema[8.1].define(version: 2026_09_18_000000) do
  create_table "widgets", force: :cascade do |t|
    t.uuid "owner_id", null: false
    t.string "name", null: false
    t.index ["owner_id"], name: "index_widgets_on_owner_id"
  end
end
"#,
    );
    assert_eq!(
        out,
        vec![
            "CREATE TABLE IF NOT EXISTS widgets (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  \
             owner_id TEXT NOT NULL,\n  name TEXT NOT NULL\n)",
            "CREATE INDEX IF NOT EXISTS index_widgets_on_owner_id ON widgets (owner_id)",
        ]
    );
    // And the DDL is applicable — the whole complaint. The crate has
    // no sqlite binding; the system CLI is the oracle, skipped where
    // there is none.
    let Ok(mut child) = std::process::Command::new("sqlite3")
        .arg(":memory:")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    else {
        return;
    };
    use std::io::Write;
    let script = out.iter().map(|s| format!("{s};\n")).collect::<String>();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let done = child.wait_with_output().unwrap();
    assert!(
        done.status.success() && done.stderr.is_empty(),
        "sqlite3 rejected the DDL: {}",
        String::from_utf8_lossy(&done.stderr)
    );
}

#[test]
fn uuid_primary_key_is_a_text_key_not_an_autoincrement() {
    roundhouse::ingest::survey::activate();
    let out = ddl(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", id: :uuid, default: -> { "gen_random_uuid()" }, force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
    );
    let gaps = roundhouse::ingest::survey::drain();
    assert_eq!(
        out[0],
        "CREATE TABLE IF NOT EXISTS widgets (\n  id TEXT PRIMARY KEY NOT NULL,\n  name TEXT NOT NULL\n)"
    );
    // A non-integer key is a schema fact, not an ingest gap: the
    // analyzer types `id` from it and the ruby emit's insert writes
    // and answers it (#90). The targets whose model layer still pins
    // an integer id report that per target at emit time
    // (`tests/primary_key_type.rs`).
    assert_eq!(gaps.len(), 0, "{gaps:?}");
}

#[test]
fn named_string_primary_key_is_the_declared_column() {
    roundhouse::ingest::survey::activate();
    let out = ddl(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "x", primary_key: "identifier", id: :string, force: :cascade do |t|
    t.string "name"
  end
end
"#,
    );
    let gaps = roundhouse::ingest::survey::drain();
    assert_eq!(
        out[0],
        "CREATE TABLE IF NOT EXISTS x (\n  identifier TEXT PRIMARY KEY NOT NULL,\n  name TEXT\n)"
    );
    // Not an ingest gap either (see the uuid case above).
    assert_eq!(gaps.len(), 0, "{gaps:?}");
}

#[test]
fn an_unknown_column_type_is_a_diagnostic_not_a_silent_drop() {
    let err = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "geo", force: :cascade do |t|
    t.st_point "location"
    t.index ["location"], name: "index_geo_on_location"
  end
end
"#,
        "db/schema.rb",
    )
    .expect_err("strict ingest must fail on a column it cannot model");
    assert!(err.to_string().contains("geo.location has unsupported type `st_point`"), "{err}");

    // Survey mode: ledgered, the rest of the schema still lands.
    roundhouse::ingest::survey::activate();
    let schema = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "geo", force: :cascade do |t|
    t.st_point "location"
    t.string "label"
  end
end
"#,
        "db/schema.rb",
    )
    .expect("survey ingest keeps going");
    let gaps = roundhouse::ingest::survey::drain();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    let cols: Vec<&str> = schema.tables.values().next().unwrap().columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(cols, vec!["id", "label"]);
}

#[test]
fn postgres_types_map_to_their_sqlite_storage() {
    let out = ddl(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "accounts", force: :cascade do |t|
    t.jsonb "fields"
    t.inet "last_ip"
    t.citext "username", null: false
    t.timestamptz "seen_at"
    t.check_constraint "length(username) > 0", name: "username_present"
  end
end
"#,
    );
    assert_eq!(
        out[0],
        "CREATE TABLE IF NOT EXISTS accounts (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  \
         fields TEXT,\n  last_ip TEXT,\n  username TEXT NOT NULL,\n  seen_at TEXT\n)"
    );
}

/// `t.timestamp` is Rails' alias for `datetime`, and what a MySQL
/// `TIMESTAMP` column dumps as (lobsters' `story_texts.created_at`).
/// It was not in the type table, so the #83 contract — an unlisted
/// type is an error — turned every lobsters lane red at ingest.
#[test]
fn timestamp_is_a_datetime() {
    let out = ddl(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "story_texts", force: :cascade do |t|
    t.timestamp "created_at", default: -> { "DATETIME('now')" }, null: false
  end
end
"#,
    );
    assert_eq!(
        out[0],
        "CREATE TABLE IF NOT EXISTS story_texts (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  \
         created_at TEXT NOT NULL\n)"
    );
}

/// The key forms Rails' PostgreSQL schema dumper writes. `id: :serial`
/// is what `id: :integer` creates there (the dumper's own spelling of
/// an integer key with a sequence); it was an unsupported type, so
/// strict ingest stopped. A key with options beyond its type and
/// default is dumped as a hash, `id: { type: :string, limit: 32 }`;
/// that was read as no option at all, so the table got an integer
/// autoincrement key in place of the string key it declared. A hash
/// with no `type:` is the default key, and `type: :integer, limit: 8`
/// is a `bigint` one, as Rails creates it.
#[test]
fn postgres_dumped_key_forms_are_the_keys_they_name() {
    use roundhouse::emit::shared::schema_sql::{render_schema_statements_for, Dialect};
    let schema_rb = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "legacy_posts", id: :serial, force: :cascade do |t|
    t.string "title"
  end
  create_table "events", id: :bigserial, force: :cascade do |t|
    t.string "name"
  end
  create_table "tokens", id: { type: :string, limit: 32 }, force: :cascade do |t|
    t.bigint "user_id", null: false
  end
  create_table "codes", primary_key: "code", id: { type: :string, limit: 8 }, force: :cascade do |t|
    t.string "label"
  end
  create_table "notes", id: { comment: "the default key, with a comment" }, force: :cascade do |t|
    t.text "body"
  end
  create_table "events_log", id: { type: :integer, limit: 8 }, force: :cascade do |t|
    t.text "payload"
  end
  create_table "outer_limit", id: { type: :integer }, limit: 8, force: :cascade do |t|
    t.text "payload"
  end
  create_table "seven_bytes", id: { type: :integer, limit: 7 }, force: :cascade do |t|
    t.text "payload"
  end
  create_table "seven_bytes_no_sequence", id: { type: :integer, limit: 7, default: nil }, force: :cascade do |t|
    t.text "payload"
  end
  create_table "inner_nil", id: { type: :integer, limit: nil }, limit: 8, force: :cascade do |t|
    t.text "payload"
  end
end
"#;
    let schema = ingest_schema(schema_rb.as_bytes(), "db/schema.rb").expect("strict ingest");
    let first_lines = |dialect: Dialect| -> Vec<String> {
        render_schema_statements_for(&schema, dialect)
            .expect("render")
            .iter()
            .map(|s| s.lines().take(2).collect::<Vec<_>>().join(" "))
            .collect()
    };
    assert_eq!(
        first_lines(Dialect::Sqlite),
        vec![
            "CREATE TABLE IF NOT EXISTS legacy_posts (   id INTEGER PRIMARY KEY AUTOINCREMENT,",
            "CREATE TABLE IF NOT EXISTS events (   id INTEGER PRIMARY KEY AUTOINCREMENT,",
            "CREATE TABLE IF NOT EXISTS tokens (   id TEXT PRIMARY KEY NOT NULL,",
            "CREATE TABLE IF NOT EXISTS codes (   code TEXT PRIMARY KEY NOT NULL,",
            "CREATE TABLE IF NOT EXISTS notes (   id INTEGER PRIMARY KEY AUTOINCREMENT,",
            "CREATE TABLE IF NOT EXISTS events_log (   id INTEGER PRIMARY KEY AUTOINCREMENT,",
            "CREATE TABLE IF NOT EXISTS outer_limit (   id INTEGER PRIMARY KEY AUTOINCREMENT,",
            "CREATE TABLE IF NOT EXISTS seven_bytes (   id INTEGER PRIMARY KEY AUTOINCREMENT,",
            "CREATE TABLE IF NOT EXISTS seven_bytes_no_sequence (   id INTEGER PRIMARY KEY AUTOINCREMENT,",
            "CREATE TABLE IF NOT EXISTS inner_nil (   id INTEGER PRIMARY KEY AUTOINCREMENT,",
        ]
    );
    assert_eq!(
        first_lines(Dialect::Postgres),
        vec![
            "CREATE TABLE IF NOT EXISTS \"legacy_posts\" (   \"id\" serial PRIMARY KEY,",
            "CREATE TABLE IF NOT EXISTS \"events\" (   \"id\" bigserial PRIMARY KEY,",
            "CREATE TABLE IF NOT EXISTS \"tokens\" (   \"id\" character varying(32) PRIMARY KEY NOT NULL,",
            "CREATE TABLE IF NOT EXISTS \"codes\" (   \"code\" character varying(8) PRIMARY KEY NOT NULL,",
            "CREATE TABLE IF NOT EXISTS \"notes\" (   \"id\" bigserial PRIMARY KEY,",
            "CREATE TABLE IF NOT EXISTS \"events_log\" (   \"id\" bigserial PRIMARY KEY,",
            // Rails merges the hash over `create_table`'s own options.
            "CREATE TABLE IF NOT EXISTS \"outer_limit\" (   \"id\" bigserial PRIMARY KEY,",
            // A sequence-backed key is `bigserial` only at `limit: 8` …
            "CREATE TABLE IF NOT EXISTS \"seven_bytes\" (   \"id\" serial PRIMARY KEY,",
            // … and with an explicit `default:` it is a 7-byte `bigint`.
            // The IR keeps no key default, so it renders the bigint key's
            // convention, `bigserial`.
            "CREATE TABLE IF NOT EXISTS \"seven_bytes_no_sequence\" (   \"id\" bigserial PRIMARY KEY,",
            "CREATE TABLE IF NOT EXISTS \"inner_nil\" (   \"id\" serial PRIMARY KEY,",
        ]
    );

    // The migration fold reads `create_table` the same way.
    use roundhouse::schema::ColumnType;
    let mut folded = roundhouse::Schema::default();
    roundhouse::ingest::ingest_migration(
        br#"class CreateKeys < ActiveRecord::Migration[8.1]
  def change
    create_table :legacy_posts, id: :serial do |t|
      t.string :title
    end
    create_table :tokens, id: { type: :string, limit: 32 } do |t|
      t.bigint :user_id
    end
    create_table :events_log, id: { type: :integer, limit: 8 } do |t|
      t.text :payload
    end
    create_table :outer_limit, id: :integer, limit: 8 do |t|
      t.text :payload
    end
  end
end
"#,
        "db/migrate/1_create_keys.rb",
        &mut folded,
    )
    .expect("fold");
    let key_types: Vec<(&str, &ColumnType)> = folded
        .tables
        .values()
        .map(|t| {
            let key = t.columns.iter().find(|c| c.primary_key).expect("a key");
            (t.name.as_str(), &key.col_type)
        })
        .collect();
    assert_eq!(
        key_types,
        vec![
            ("legacy_posts", &ColumnType::Integer),
            ("tokens", &ColumnType::String { limit: Some(32) }),
            ("events_log", &ColumnType::BigInt),
            ("outer_limit", &ColumnType::BigInt),
        ]
    );
}

/// `t.serial` as an ordinary column stays an error: outside the key,
/// nothing in either dialect would fill its sequence.
#[test]
fn a_serial_column_outside_the_key_is_still_unsupported() {
    let err = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "tickets", force: :cascade do |t|
    t.serial "number", null: false
  end
end
"#,
        "db/schema.rb",
    )
    .expect_err("a serial column is not a key");
    assert!(err.to_string().contains("tickets.number has unsupported type `serial`"), "{err}");
}
