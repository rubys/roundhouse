//! Target-neutral schema DDL rendering.
//!
//! Produces a single CREATE TABLE ... string covering every table in the
//! ingested `Schema`. Each per-target emitter embeds the result in its
//! generated project (e.g. a `pub const CREATE_TABLES: &str` in Rust,
//! a `CREATE_TABLES = """ ... """` in Python) so a fresh `:memory:`
//! SQLite connection can initialize via `execute_batch(CREATE_TABLES)`.
//!
//! Lives under `emit/shared/` rather than `lower/` because it produces
//! final target text, not a structured lower-level IR — the rest of
//! `lower/` is structured-to-structured in the compiler sense.
//!
//! SQLite-specific today. When a target demands Postgres/MySQL, the
//! per-dialect rendering moves behind a `Dialect` enum — the `Schema`
//! IR itself stays dialect-neutral.

use std::fmt::Write;

use crate::schema::{ColumnType, Schema};

/// Render every table + index in `schema` as a list of SQLite DDL
/// statements — one CREATE TABLE per table, one CREATE INDEX per
/// index. Idempotent (`IF NOT EXISTS`) so the runtime can re-run
/// against an existing DB without erroring.
///
/// Statements-list shape (rather than one joined string) is the
/// general form: portable across DB drivers that don't support
/// multi-statement execution (Postgres' pg gem, MySQL drivers),
/// and gives clearer per-statement error reporting in any adapter.
/// Adapters that DO accept multi-statement (better-sqlite3) just
/// `join("\n")` the list.
///
/// Primary keys use `INTEGER PRIMARY KEY AUTOINCREMENT` so rowids
/// stay stable and monotonic across inserts — matching what the
/// Rails sqlite3 adapter emits for the default `id` column.
pub fn render_schema_statements(schema: &Schema) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_name, table) in &schema.tables {
        let mut s = String::new();
        writeln!(s, "CREATE TABLE IF NOT EXISTS {} (", table.name.as_str()).unwrap();
        let mut lines: Vec<String> = Vec::new();
        for col in &table.columns {
            let mut line = String::new();
            if col.primary_key {
                line.push_str(&format!(
                    "  {} INTEGER PRIMARY KEY AUTOINCREMENT",
                    col.name.as_str()
                ));
            } else {
                line.push_str(&format!(
                    "  {} {}",
                    col.name.as_str(),
                    sqlite_type(&col.col_type)
                ));
                if !col.nullable {
                    line.push_str(" NOT NULL");
                }
            }
            lines.push(line);
        }
        writeln!(s, "{}", lines.join(",\n")).unwrap();
        s.push(')');
        out.push(s);
    }
    for (_name, table) in &schema.tables {
        for idx in &table.indexes {
            let cols: Vec<&str> = idx.columns.iter().map(|c| c.as_str()).collect();
            let unique = if idx.unique { "UNIQUE " } else { "" };
            out.push(format!(
                "CREATE {unique}INDEX IF NOT EXISTS {} ON {} ({})",
                idx.name.as_str(),
                table.name.as_str(),
                cols.join(", "),
            ));
        }
    }
    out
}

/// Joined-string form of `render_schema_statements` — kept for
/// per-target emitters that embed schema as a single `const` /
/// `let` declaration (Rust, Go, Python, Elixir, Crystal). Each
/// statement is `;`-terminated and joined with newlines so a single
/// `db.exec(joined)` call against a multi-statement-supporting
/// adapter executes them all.
pub fn render_schema_sql(schema: &Schema) -> String {
    let mut s = String::new();
    for stmt in render_schema_statements(schema) {
        s.push_str(&stmt);
        s.push_str(";\n");
    }
    s
}

/// Map a Roundhouse `ColumnType` to a SQLite storage class. SQLite's
/// type system is looser than most SQL engines; these mappings follow
/// what the Rails sqlite3 adapter emits so stored values round-trip
/// through both stacks.
fn sqlite_type(ct: &ColumnType) -> &'static str {
    match ct {
        ColumnType::Integer | ColumnType::BigInt => "INTEGER",
        ColumnType::Float | ColumnType::Decimal { .. } => "REAL",
        ColumnType::Boolean => "INTEGER",
        ColumnType::Binary => "BLOB",
        ColumnType::String { .. }
        | ColumnType::Text
        | ColumnType::Date
        | ColumnType::DateTime
        | ColumnType::Time
        | ColumnType::Json => "TEXT",
        ColumnType::Reference { .. } => "INTEGER",
    }
}
