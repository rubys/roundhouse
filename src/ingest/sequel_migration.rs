//! Sequel migration ingestion — folds one `db/migrate/*.rb` written in
//! Sequel's migration DSL into the shared [`Schema`], the same target
//! `ingest_migration` (Rails) folds into. Part of the Roda + Sequel
//! front-end (issue #67); the mapping table lives in
//! `docs/roda-sequel-plan.md`.
//!
//! Recognized shape:
//!
//! ```ruby
//! Sequel.migration do
//!   change do            # or `up do`
//!     create_table(:articles) do
//!       primary_key :id
//!       String :title, null: false
//!       String :body, text: true, null: false
//!       DateTime :created_at, null: false
//!       foreign_key :article_id, :articles, null: false, on_delete: :cascade
//!       index :article_id
//!     end
//!   end
//! end
//! ```
//!
//! Unrecognized statements inside a table block are recorded as ledger
//! entries (survey mode) or abort (strict mode) — never silently
//! dropped, per the routes.rs "ledger line, never a silently empty
//! table" convention.

use ruby_prism::Node;

use crate::schema::{
    Column, ColumnType, ForeignKey, Index, ReferentialAction, Schema, Table,
};
use crate::{Symbol, TableRef};

use super::util::{bool_value, constant_id_str, flatten_statements, symbol_value};
use super::{IngestError, IngestResult};

/// Fold one Sequel migration file into `schema`. Files are applied in
/// filename order by the caller (Sequel's integer prefixes sort the
/// same way Rails' timestamps do).
pub fn ingest_sequel_migration(
    source: &[u8],
    file: &str,
    schema: &mut Schema,
) -> IngestResult<()> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(program) = root.as_program_node() else {
        return Err(IngestError::Parse {
            file: file.into(),
            message: "migration is not a program".into(),
        });
    };

    for stmt in program.statements().body().iter() {
        let Some(call) = stmt.as_call_node() else { continue };
        // `Sequel.migration do ... end`
        let is_sequel_migration = constant_id_str(&call.name()) == "migration"
            && call
                .receiver()
                .and_then(|r| r.as_constant_read_node().map(|c| c.name()))
                .is_some_and(|n| constant_id_str(&n) == "Sequel");
        if !is_sequel_migration {
            continue;
        }
        let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
        else {
            continue;
        };
        for inner in flatten_statements(body) {
            let Some(dir) = inner.as_call_node() else { continue };
            // `change do ... end` / `up do ... end` (down is ignored:
            // the fold only ever applies forward).
            if !matches!(constant_id_str(&dir.name()), "change" | "up") {
                continue;
            }
            let Some(dir_body) =
                dir.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
            else {
                continue;
            };
            for table_stmt in flatten_statements(dir_body) {
                ingest_table_stmt(&table_stmt, file, schema)?;
            }
        }
    }
    Ok(())
}

fn ingest_table_stmt(
    stmt: &Node<'_>,
    file: &str,
    schema: &mut Schema,
) -> IngestResult<()> {
    let Some(call) = stmt.as_call_node() else { return Ok(()) };
    match constant_id_str(&call.name()) {
        "create_table" => {
            let name = call
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .and_then(|n| symbol_value(&n))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: "create_table without a symbol table name".into(),
                })?;
            let mut table = Table {
                name: Symbol::from(name.as_str()),
                columns: Vec::new(),
                indexes: Vec::new(),
                foreign_keys: Vec::new(),
            };
            if let Some(body) =
                call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
            {
                for col_stmt in flatten_statements(body) {
                    ingest_column_stmt(&col_stmt, file, &mut table)?;
                }
            }
            schema.tables.insert(Symbol::from(name.as_str()), table);
        }
        // alter_table / drop_table land when a fixture forces them.
        other => {
            let err = IngestError::Unsupported {
                file: file.into(),
                message: format!("sequel migration directive not recognized: {other}"),
            };
            if !super::survey::is_active() {
                return Err(err);
            }
            super::survey::record(&err);
        }
    }
    Ok(())
}

/// One statement inside a `create_table` block: a type-named column
/// call (`String :title, null: false`), `primary_key`, `foreign_key`,
/// or `index`.
fn ingest_column_stmt(
    stmt: &Node<'_>,
    file: &str,
    table: &mut Table,
) -> IngestResult<()> {
    let Some(call) = stmt.as_call_node() else { return Ok(()) };
    let method = constant_id_str(&call.name()).to_string();
    let args: Vec<Node<'_>> = call
        .arguments()
        .map(|a| a.arguments().iter().collect())
        .unwrap_or_default();
    let col_name = args.first().and_then(symbol_value);
    let opts = args.iter().find_map(|a| a.as_keyword_hash_node());

    // Kwarg readers over the trailing options hash.
    let opt_bool = |key: &str| -> Option<bool> {
        let kh = opts.as_ref()?;
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            if symbol_value(&assoc.key()).as_deref() == Some(key) {
                return bool_value(&assoc.value());
            }
        }
        None
    };
    let opt_sym = |key: &str| -> Option<String> {
        let kh = opts.as_ref()?;
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            if symbol_value(&assoc.key()).as_deref() == Some(key) {
                return symbol_value(&assoc.value());
            }
        }
        None
    };

    match method.as_str() {
        "primary_key" => {
            let name = col_name.ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "primary_key without a column name".into(),
            })?;
            table.columns.push(Column {
                name: Symbol::from(name.as_str()),
                col_type: ColumnType::Integer,
                nullable: false,
                default: None,
                primary_key: true,
            });
        }
        "foreign_key" => {
            // `foreign_key :article_id, :articles, null: false, on_delete: :cascade`
            let name = col_name.ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "foreign_key without a column name".into(),
            })?;
            let target = args.get(1).and_then(symbol_value).ok_or_else(|| {
                IngestError::Unsupported {
                    file: file.into(),
                    message: format!("foreign_key :{name} without a target table"),
                }
            })?;
            table.columns.push(Column {
                name: Symbol::from(name.as_str()),
                col_type: ColumnType::Reference {
                    table: TableRef(Symbol::from(target.as_str())),
                },
                nullable: opt_bool("null").unwrap_or(true),
                default: None,
                primary_key: false,
            });
            table.foreign_keys.push(ForeignKey {
                from_column: Symbol::from(name.as_str()),
                to_table: TableRef(Symbol::from(target.as_str())),
                to_column: Symbol::from("id"),
                on_delete: referential_action(opt_sym("on_delete").as_deref()),
                on_update: referential_action(opt_sym("on_update").as_deref()),
            });
        }
        "index" => {
            let columns: Vec<Symbol> = match args.first() {
                Some(n) if n.as_array_node().is_some() => n
                    .as_array_node()
                    .expect("checked")
                    .elements()
                    .iter()
                    .filter_map(|e| symbol_value(&e))
                    .map(|s| Symbol::from(s.as_str()))
                    .collect(),
                Some(n) => symbol_value(n)
                    .map(|s| vec![Symbol::from(s.as_str())])
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            if columns.is_empty() {
                return Ok(());
            }
            // Rails-convention synthetic name, so the folded schema
            // diffs cleanly against a schema.rb ingest.
            let name = format!(
                "index_{}_on_{}",
                table.name.as_str(),
                columns
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join("_and_")
            );
            table.indexes.push(Index {
                name: Symbol::from(name),
                columns,
                unique: opt_bool("unique").unwrap_or(false),
            });
        }
        // Type-named column call: `String :title`, `DateTime :created_at`.
        type_name => {
            let Some(col_type) = column_type(type_name, opt_bool("text").unwrap_or(false))
            else {
                let err = IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "sequel column shape not recognized in create_table(:{}): {type_name}",
                        table.name
                    ),
                };
                if !super::survey::is_active() {
                    return Err(err);
                }
                super::survey::record(&err);
                return Ok(());
            };
            let name = col_name.ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: format!("{type_name} column without a name"),
            })?;
            table.columns.push(Column {
                name: Symbol::from(name.as_str()),
                col_type,
                nullable: opt_bool("null").unwrap_or(true),
                default: None,
                primary_key: false,
            });
        }
    }
    Ok(())
}

/// Sequel's Ruby-class-named column types → shared `ColumnType`.
/// `String :x, text: true` is Sequel's spelling of a TEXT column.
fn column_type(name: &str, text: bool) -> Option<ColumnType> {
    Some(match name {
        "String" if text => ColumnType::Text,
        "String" => ColumnType::String { limit: None },
        "Integer" | "Fixnum" => ColumnType::Integer,
        "Bignum" => ColumnType::BigInt,
        "Float" => ColumnType::Float,
        "BigDecimal" | "Numeric" => ColumnType::Decimal { precision: None, scale: None },
        "Date" => ColumnType::Date,
        "DateTime" | "Timestamp" => ColumnType::DateTime,
        "Time" => ColumnType::Time,
        "TrueClass" | "FalseClass" => ColumnType::Boolean,
        "File" => ColumnType::Binary,
        _ => return None,
    })
}

fn referential_action(sym: Option<&str>) -> ReferentialAction {
    match sym {
        Some("cascade") => ReferentialAction::Cascade,
        Some("restrict") => ReferentialAction::Restrict,
        Some("set_null") => ReferentialAction::SetNull,
        Some("set_default") => ReferentialAction::SetDefault,
        Some("no_action") | None => ReferentialAction::NoAction,
        Some(_) => ReferentialAction::NoAction,
    }
}
