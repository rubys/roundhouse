//! `db/schema.rb` — parse the Rails schema DSL (`ActiveRecord::Schema`
//! plus a sequence of `create_table`s with column calls) into a
//! target-neutral `Schema`. Rails' implicit bigint `id` primary key is
//! synthesized here unless `id: false` is passed to `create_table`.

use ruby_prism::parse;

use crate::schema::{Column, ColumnType, Index, Schema, Table};
use crate::{Symbol, TableRef};

use super::IngestResult;
use super::util::{
    bool_value, constant_id_str, flatten_statements, integer_value, string_value, symbol_value,
    walk_calls,
};

pub fn ingest_schema(source: &[u8], _file: &str) -> IngestResult<Schema> {
    let result = parse(source);
    let root = result.node();

    let mut schema = Schema::default();
    walk_calls(&root, &mut |call| {
        if constant_id_str(&call.name()) != "create_table" {
            return;
        }
        let Some(args) = call.arguments() else { return };
        let first = args.arguments().iter().next();
        let Some(table_name) = first.as_ref().and_then(string_value) else { return };

        // Rails convention: every table has an implicit bigint primary-key `id`
        // unless `id: false` is passed to `create_table`. We honor that here by
        // synthesizing the column; the Ruby emitter's `primary_key` skip keeps
        // schema.rb round-trip-equal to the source.
        let mut has_id = true;
        let create_table_args = args.arguments();
        for arg in create_table_args.iter().skip(1) {
            let Some(kh) = arg.as_keyword_hash_node() else { continue };
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                if key.as_str() == "id" {
                    if let Some(false) = bool_value(&assoc.value()) {
                        has_id = false;
                    }
                }
            }
        }

        let mut columns = Vec::new();
        let mut indexes: Vec<Index> = Vec::new();
        if has_id {
            columns.push(Column {
                name: Symbol::from("id"),
                col_type: ColumnType::BigInt,
                nullable: false,
                default: None,
                primary_key: true,
            });
        }
        if let Some(block_node) = call.block() {
            if let Some(block) = block_node.as_block_node() {
                if let Some(body) = block.body() {
                    for stmt in flatten_statements(body) {
                        if let Some(call) = stmt.as_call_node() {
                            let call_name = constant_id_str(&call.name()).to_string();
                            if call_name == "index" {
                                if let Some(idx) = index_from_call(&call, &table_name) {
                                    indexes.push(idx);
                                }
                            } else if let Some(col) = column_from_call(&call) {
                                columns.push(col);
                            }
                        }
                    }
                }
            }
        }

        schema.tables.insert(
            Symbol::from(table_name.clone()),
            Table {
                name: Symbol::from(table_name),
                columns,
                indexes,
                foreign_keys: vec![],
            },
        );
    });

    Ok(schema)
}

fn column_from_call(call: &ruby_prism::CallNode<'_>) -> Option<Column> {
    // Expected: t.string "title", null: false
    // Receiver is a LocalVariableReadNode named "t".
    let recv = call.receiver()?;
    recv.as_local_variable_read_node()?;

    let col_type_name = constant_id_str(&call.name()).to_string();
    let args_node = call.arguments()?;
    let first = args_node.arguments().iter().next()?;
    let col_name = string_value(&first)?;

    let mut nullable = true;
    let mut default: Option<String> = None;
    let mut limit: Option<u32> = None;

    for arg in args_node.arguments().iter().skip(1) {
        if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                let value = &assoc.value();
                match key.as_str() {
                    "null" => nullable = bool_value(value).unwrap_or(true),
                    "default" => default = string_value(value),
                    "limit" => {
                        if let Some(n) = integer_value(value) {
                            if n >= 0 {
                                limit = Some(n as u32);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let col_type = match col_type_name.as_str() {
        "integer" => ColumnType::Integer,
        "bigint" => ColumnType::BigInt,
        "float" => ColumnType::Float,
        "decimal" => ColumnType::Decimal { precision: None, scale: None },
        "string" => ColumnType::String { limit },
        "text" => ColumnType::Text,
        "boolean" => ColumnType::Boolean,
        "date" => ColumnType::Date,
        "datetime" => ColumnType::DateTime,
        "time" => ColumnType::Time,
        "binary" => ColumnType::Binary,
        "json" => ColumnType::Json,
        "references" => ColumnType::Reference { table: TableRef(Symbol::from(col_name.as_str())) },
        _ => return None,
    };

    Some(Column {
        name: Symbol::from(col_name),
        col_type,
        nullable,
        default,
        primary_key: false,
    })
}

fn index_from_call(call: &ruby_prism::CallNode<'_>, table_name: &str) -> Option<Index> {
    // Expected: t.index ["article_id"], name: "...", unique: true
    let recv = call.receiver()?;
    recv.as_local_variable_read_node()?;

    let args_node = call.arguments()?;
    let first = args_node.arguments().iter().next()?;
    let arr = first.as_array_node()?;

    let mut columns: Vec<Symbol> = Vec::new();
    for el in arr.elements().iter() {
        if let Some(name) = string_value(&el) {
            columns.push(Symbol::from(name));
        }
    }
    if columns.is_empty() {
        return None;
    }

    let mut explicit_name: Option<String> = None;
    let mut unique = false;
    for arg in args_node.arguments().iter().skip(1) {
        if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                let value = &assoc.value();
                match key.as_str() {
                    "name" => explicit_name = string_value(value),
                    "unique" => unique = bool_value(value).unwrap_or(false),
                    _ => {}
                }
            }
        }
    }

    let name = explicit_name.unwrap_or_else(|| {
        let cols: Vec<&str> = columns.iter().map(|c| c.as_str()).collect();
        format!("index_{}_on_{}", table_name, cols.join("_and_"))
    });

    Some(Index {
        name: Symbol::from(name),
        columns,
        unique,
    })
}
