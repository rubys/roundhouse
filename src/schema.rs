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
    pub generated_int4_compatible: Option<bool>,
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

/// One `db/migrate/<version>_<name>.rb`, as `rails db:migrate` runs it:
/// the version it records in `schema_migrations` and the schema changes
/// it makes, in order. Kept beside the folded [`Schema`] (which only
/// says where the migrations end up) so an emitted app can bring an
/// existing database up to date, as Rails' `db:migrate` does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Migration {
    /// The filename's leading digits (`20260913000001`).
    pub version: String,
    /// The migration's file, for messages.
    pub file: String,
    pub steps: Vec<MigrationStep>,
    /// Why the steps above are not the whole migration: a verb the fold
    /// does not model, or Ruby beside the verbs (a backfill). Such a
    /// migration is not run by the emitted `db:migrate` (it stops and
    /// names it), since recording it as applied would claim an effect it
    /// never had; a fresh database still gets its end state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_renderable: Option<String>,
    /// Whether the app's schema (`Schema.statements`) already has its
    /// changes: loading the schema records it as applied, as Rails'
    /// `assume_migrated_upto_version` does with schema.rb's `version:`. A
    /// migration newer than schema.rb is not, and `db:prepare` runs it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_schema: bool,
}

/// A schema change as a database applies it. The tables carried are the
/// state the change leaves (and, for a rebuild, the state before), so the
/// DDL is rendered from the same definitions the fold computed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MigrationStep {
    /// `create_table`, with the indexes declared in its block.
    CreateTable { table: Table },
    DropTable { name: Symbol },
    RenameTable { from: Symbol, to: Symbol },
    /// `add_column`, `add_timestamps`, `add_reference`: `ALTER TABLE ...
    /// ADD COLUMN`, one column each.
    AddColumns { table: Symbol, columns: Vec<Column> },
    RenameColumn { table: Symbol, from: Symbol, to: Symbol },
    /// A change SQLite cannot make in place (`remove_column`,
    /// `change_column`, `change_column_null`, `change_column_default`):
    /// the table is copied into its new definition, as Rails' SQLite
    /// adapter does (`alter_table`).
    RebuildTable { before: Table, after: Table },
    /// An index the change added (`add_index`, `add_reference`'s, a
    /// renamed one under its new name), with the table it indexes.
    AddIndex { table: Table, index: Index },
    /// An index the change removed (or a renamed one's old name).
    RemoveIndex { name: Symbol },
    /// `execute "UPDATE ..."`: SQL that changes data and not the schema,
    /// run as written.
    Execute { sql: String },
}
