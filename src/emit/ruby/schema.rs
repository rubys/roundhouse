//! `db/schema.rb` emission.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;
use crate::schema::{Column, ColumnType, Schema, Table};

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
