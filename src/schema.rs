//! The database-schema IR: tables, columns, indexes, and foreign keys
//! in target-neutral form. `ingest::schema` builds it from
//! `db/schema.rb` — or by folding `db/migrate/*.rb` in timestamp order
//! when no schema.rb ships — and `ingest::sequel_migration` produces
//! the same shape for non-Rails apps. This is the pipeline's root
//! type-evidence source: model ingest derives each model's attribute
//! types from its table's columns, so a column's `ColumnType` and
//! `nullable` flag decide the `Ty` (and the `T | Nil` unions) every
//! target ultimately emits.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::ident::{Symbol, TableRef};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    pub tables: IndexMap<Symbol, Table>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub name: Symbol,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: Symbol,
    pub col_type: ColumnType,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColumnType {
    Integer,
    BigInt,
    Float,
    Decimal { precision: Option<u8>, scale: Option<u8> },
    String { limit: Option<u32> },
    Text,
    Boolean,
    Date,
    DateTime,
    Time,
    Binary,
    Json,
    Reference { table: TableRef },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub name: Symbol,
    pub columns: Vec<Symbol>,
    pub unique: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub from_column: Symbol,
    pub to_table: TableRef,
    pub to_column: Symbol,
    pub on_delete: ReferentialAction,
    pub on_update: ReferentialAction,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferentialAction {
    #[default]
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}
