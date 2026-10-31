//! The database-schema IR: tables, columns, indexes, and foreign keys
//! in target-neutral form. `ingest::schema` builds it from
//! `db/schema.rb` — or by folding `db/migrate/*.rb` in timestamp order
//! when no schema.rb ships — and `ingest::sequel_migration` produces
//! the same shape for non-Rails apps. This is the pipeline's root
//! type-evidence source: model ingest derives each model's attribute
//! types from its table's columns, so a column's `ColumnType` and
//! `nullable` flag decide the `Ty` (and the `T | Nil` unions) every
//! target ultimately emits.

pub mod generated;

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
    /// A plain `create_table id: false` declaration with no later key or
    /// id-column changes. False also means unknown: retain the default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub known_keyless: bool,
    /// A composite primary key's columns in key order (structure.sql's
    /// `PRIMARY KEY (shop_id, id)`). No column is flagged `primary_key`
    /// for one: a model keys its records on its declared
    /// `self.primary_key` (`ingest::app::key_composite_tables`), and a
    /// model that declares none is refused, since Rails keys it on every
    /// column here and this compiler does not yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub primary_key_columns: Vec<Symbol>,
    /// `create_virtual_table "message_search_index", "fts5", ["body",
    /// "tokenize=porter"]` — a table the DB builds from a MODULE rather
    /// than from a column list. It has no rowid column of its own, no
    /// types, and no indexes, so the DDL renderer takes a different
    /// branch entirely; everything else about it is a table, which is
    /// why it lives here rather than in a parallel collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_module: Option<VirtualModule>,
}

/// The module and argument list of a `create_virtual_table`. Rendered
/// back verbatim — the arguments are the module's own DSL (fts5 takes
/// column names AND `tokenize=…` options in one list), so parsing them
/// into anything finer would be inventing a grammar per module.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualModule {
    pub module: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: Symbol,
    pub col_type: ColumnType,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    /// The SQL expression and storage mode of a generated column. The
    /// declared `col_type` remains the column's result type; generated
    /// columns are not writable by application inserts or updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<GeneratedColumn>,
    /// Ingest-only evidence for generated-expression validation. Some
    /// database types (for example `inet`, PostgreSQL enums, and fixed
    /// `character`) normalize to `String { limit: None }` or `Text` for
    /// Roundhouse's ordinary-column typing. `Some(false)` records that
    /// the original type does not have the portable text semantics
    /// required by the supported generated-expression grammar. The same
    /// negative evidence also guards custom-qualified JSON types after
    /// normalization: their built-in JSON semantics are not proven. `None`
    /// means the normalized `col_type` is sufficient evidence. This is
    /// sparse so well-represented ordinary types retain their serialized form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_text_compatible: Option<bool>,
    /// Ingest-only evidence for generated integer-expression validation.
    /// Integer-like source types such as `smallint`, `serial`, and types
    /// with discarded typmods normalize to `ColumnType::Integer`, but are
    /// not faithful PostgreSQL `int4` results. `Some(false)` retains that
    /// negative evidence; `None` means the normalized type is sufficient.
    /// This stays sparse so ordinary integer columns keep their serialized
    /// form and application typing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_int4_compatible: Option<bool>,    /// A PostgreSQL array column (`t.bigint "tag_ids", array: true`):
    /// Rails reads an `Array` of `col_type`, or nil when the column is
    /// nullable and unset. `col_type` stays the element type, as Rails'
    /// own column metadata keeps it. Sparse, so a non-array column keeps
    /// its serialized form.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub array: bool,
    /// The target stores this array column as an Array: set by
    /// `lower::array_columns::store_arrays` on the ruby family, where it
    /// is typed `Array[<element>]` and kept in SQLite as JSON text. Every
    /// other target reads the element, as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub array_stored: bool,
}

impl Column {
    /// The schema default a target reads. An array column's `[]` counts
    /// only where the target stores the Array.
    pub fn stored_default(&self) -> Option<&str> {
        if self.array && !self.array_stored {
            return None;
        }
        self.default.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeneratedColumn {
    /// Source SQL retained from `as:` or `GENERATED ALWAYS AS (...)`.
    pub expression: String,
    pub storage: GeneratedColumnStorage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedColumnStorage {
    Stored,
    Virtual,
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
    /// `t.json` — retained separately from PostgreSQL `jsonb` for faithful
    /// schema round-trips; shared model storage remains serialized text.
    Json,
    /// `t.jsonb` — distinct from `t.json` for faithful PostgreSQL schema
    /// round-trips. Both JSON variants share serialized-text model storage.
    Jsonb,
    /// `t.uuid` — a Postgres `uuid` column. SQLite has no uuid type, so
    /// storage is TEXT (the 36-char canonical form); typing is a String.
    /// Not modeled as `String` at ingest so a schema round-trip and a
    /// per-dialect renderer can still tell the two apart.
    Uuid,
    /// `t.interval` — a Postgres `interval` column. SQLite stores the
    /// canonical whole-second count as INTEGER. The public reader is an
    /// `ActiveSupport::Duration`; keeping a distinct variant lets
    /// PostgreSQL DDL still say `interval`.
    Interval,
    Reference { table: TableRef },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub name: Symbol,
    pub columns: Vec<Symbol>,
    pub unique: bool,
    /// PostgreSQL's `USING <access_method>`, when present in the source.
    /// `None` is the database default (btree). The method is retained
    /// because substituting btree can fail or remove index support for queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub using: Option<String>,
    /// A partial index's predicate, the SQL of `t.index …, where:` (or
    /// of `CREATE INDEX … WHERE` in `structure.sql`) as the source
    /// database's dumper wrote it. On a unique index it decides which
    /// rows the uniqueness applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub from_column: Symbol,
    pub to_table: TableRef,
    pub to_column: Symbol,
    pub on_delete: ReferentialAction,
    pub on_update: ReferentialAction,
}

/// Column a `pluck` or `pick` argument selects.
///
/// Rails 8.1.4 selects a real `id` column from `pluck(:id)`. `:id` is
/// the primary-key alias only when the table has no column of that
/// name. `ids` does not use this: that terminal always reads the key,
/// which the caller expresses with `id_is_column: false`.
///
/// `Relation#pluck` applies the same rule against `schema_columns`.
pub fn pluck_argument(requested: Symbol, id_is_column: bool, primary_key: Option<&Symbol>) -> Symbol {
    if requested.as_str() == "id" && !id_is_column {
        primary_key.cloned().unwrap_or(requested)
    } else {
        requested
    }
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
