//! `db/schema.rb` emission.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;
use crate::schema::{Column, ColumnType, Index, Schema, Table};

pub(super) fn emit_schema(schema: &Schema) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "ActiveRecord::Schema.define do").unwrap();
    for table in schema.tables.values() {
        emit_table(&mut s, table);
    }
    for table in schema.tables.values() {
        for fk in &table.foreign_keys {
            writeln!(
                s,
                "  add_foreign_key {:?}, {:?}, column: {:?}, primary_key: {:?}",
                table.name.as_str(),
                fk.to_table.to_string(),
                fk.from_column.as_str(),
                fk.to_column.as_str(),
            )
            .unwrap();
        }
    }
    writeln!(s, "end").unwrap();
    EmittedFile { path: PathBuf::from("db/schema.rb"), content: s }
}

fn emit_table(out: &mut String, table: &Table) {
    writeln!(out, "  create_table {:?}, force: :cascade do |t|", table.name.as_str()).unwrap();
    for col in &table.columns {
        if col.primary_key {
            continue; // Rails synthesizes `id` by default.
        }
        writeln!(out, "    {}", emit_column(col)).unwrap();
    }
    for idx in &table.indexes {
        let cols: Vec<String> = idx.columns.iter().map(|c| format!("{:?}", c.as_str())).collect();
        let unique = if idx.unique { ", unique: true" } else { "" };
        writeln!(
            out,
            "    t.index [{}], name: {:?}{}",
            cols.join(", "),
            idx.name.as_str(),
            unique
        )
        .unwrap();
    }
    writeln!(out, "  end").unwrap();
}

fn emit_column(col: &Column) -> String {
    let method = match &col.col_type {
        ColumnType::Integer => "integer",
        ColumnType::BigInt => "bigint",
        ColumnType::Float => "float",
        ColumnType::Decimal { .. } => "decimal",
        ColumnType::String { .. } => "string",
        ColumnType::Text => "text",
        ColumnType::Boolean => "boolean",
        ColumnType::Date => "date",
        ColumnType::DateTime => "datetime",
        ColumnType::Time => "time",
        ColumnType::Binary => "binary",
        ColumnType::Json => "json",
        ColumnType::Reference { .. } => "references",
    };
    let mut opts: Vec<String> = Vec::new();
    if let ColumnType::String { limit: Some(n) } = &col.col_type {
        opts.push(format!("limit: {n}"));
    }
    if let ColumnType::Decimal { precision, scale } = &col.col_type {
        if let Some(p) = precision { opts.push(format!("precision: {p}")); }
        if let Some(s) = scale { opts.push(format!("scale: {s}")); }
    }
    if !col.nullable { opts.push("null: false".to_string()); }
    if let Some(d) = &col.default {
        opts.push(format!("default: {d:?}"));
    }
    if opts.is_empty() {
        format!("t.{method} {:?}", col.name.as_str())
    } else {
        format!("t.{method} {:?}, {}", col.name.as_str(), opts.join(", "))
    }
}

/// Emit `config/schema.rb` in spinel-blog shape: a `Schema` module
/// containing a frozen `STATEMENTS` array of raw `CREATE TABLE` /
/// `CREATE INDEX` strings, plus a `self.load!` that walks them through
/// the adapter. Foreign-key constraints are dropped — the framework
/// enforces relationships at the app layer (e.g. `belongs_to`'s
/// `Article.find_by(id: @article_id)` lookup), not at the DB layer.
pub(super) fn emit_lowered_schema(schema: &Schema) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "module Schema").unwrap();
    writeln!(s, "  STATEMENTS = [").unwrap();
    for table in schema.tables.values() {
        emit_create_table_heredoc(&mut s, table);
    }
    for table in schema.tables.values() {
        for idx in &table.indexes {
            emit_create_index_line(&mut s, table, idx);
        }
    }
    writeln!(s, "  ].freeze").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def self.load!(adapter)").unwrap();
    writeln!(s, "    STATEMENTS.each {{ |sql| adapter.execute_ddl(sql) }}").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile { path: PathBuf::from("config/schema.rb"), content: s }
}

fn emit_create_table_heredoc(out: &mut String, table: &Table) {
    writeln!(out, "    <<~SQL,").unwrap();
    writeln!(out, "      CREATE TABLE IF NOT EXISTS {} (", table.name.as_str()).unwrap();
    writeln!(out, "        id INTEGER PRIMARY KEY AUTOINCREMENT,").unwrap();
    let non_pk: Vec<&Column> = table.columns.iter().filter(|c| !c.primary_key).collect();
    for (i, col) in non_pk.iter().enumerate() {
        let comma = if i + 1 < non_pk.len() { "," } else { "" };
        let null_clause = if col.nullable { "" } else { " NOT NULL" };
        writeln!(
            out,
            "        {} {}{}{}",
            col.name.as_str(),
            sqlite_type(&col.col_type),
            null_clause,
            comma,
        )
        .unwrap();
    }
    writeln!(out, "      )").unwrap();
    writeln!(out, "    SQL").unwrap();
}

fn emit_create_index_line(out: &mut String, table: &Table, idx: &Index) {
    let cols: Vec<&str> = idx.columns.iter().map(|c| c.as_str()).collect();
    let unique = if idx.unique { "UNIQUE " } else { "" };
    writeln!(
        out,
        "    \"CREATE {unique}INDEX IF NOT EXISTS {} ON {} ({})\",",
        idx.name.as_str(),
        table.name.as_str(),
        cols.join(", "),
    )
    .unwrap();
}

fn sqlite_type(ct: &ColumnType) -> &'static str {
    match ct {
        ColumnType::Integer | ColumnType::BigInt | ColumnType::Reference { .. } => "INTEGER",
        ColumnType::Float => "REAL",
        ColumnType::Decimal { .. } => "NUMERIC",
        ColumnType::Boolean => "INTEGER",
        ColumnType::Binary => "BLOB",
        ColumnType::String { .. }
        | ColumnType::Text
        | ColumnType::Date
        | ColumnType::DateTime
        | ColumnType::Time
        | ColumnType::Json => "TEXT",
    }
}
