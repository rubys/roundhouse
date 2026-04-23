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

/// Render every table in `schema` as SQLite CREATE TABLE statements,
/// joined with blank lines. Primary keys use
/// `INTEGER PRIMARY KEY AUTOINCREMENT` so rowids stay stable and
/// monotonic across inserts — matching what the Rails sqlite3 adapter
/// emits for the default `id` column.
pub fn render_schema_sql(schema: &Schema) -> String {
    let mut s = String::new();
    for (_name, table) in &schema.tables {
        writeln!(s, "CREATE TABLE {} (", table.name.as_str()).unwrap();
        let mut lines: Vec<String> = Vec::new();
        for col in &table.columns {
            let mut line = String::new();
            if col.primary_key {
                // SQLite rowid alias. `INTEGER PRIMARY KEY AUTOINCREMENT`
                // matches Rails sqlite3 adapter output.
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
        writeln!(s, ");").unwrap();
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
