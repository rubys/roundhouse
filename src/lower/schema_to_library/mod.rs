//! Lower the database `Schema` into a `Schema` LibraryClass with one
//! class method that returns the joined CREATE TABLE/INDEX DDL as a
//! single String. The runtime calls `Schema.create_tables()` at
//! startup to initialize a fresh `:memory:` SQLite connection (or the
//! configured adapter). One emitted file per target.
//!
//! Self-describing IR: the SQL is materialized inside the method
//! body as a typed `Lit::Str`, so the walker emits it unchanged
//! across every target. The lowerer relies on the existing
//! `emit::shared::schema_sql::render_schema_sql` for SQLite-flavored
//! DDL — keeps DDL rendering in one place; per-dialect variants
//! land here when other databases need support.

use crate::dialect::LibraryFunction;
use crate::effect::EffectSet;
use crate::ident::Symbol;
use crate::lower::typing::{fn_sig, lit_str};
use crate::schema::Schema;
use crate::ty::Ty;

/// Build the `Schema` module as a single LibraryFunction:
/// `Schema.create_tables() -> string` returning the rendered DDL.
/// Empty when `schema.tables` is empty — apps without persisted
/// models don't need a Schema artifact.
pub fn lower_schema_to_library_functions(schema: &Schema) -> Vec<LibraryFunction> {
    if schema.tables.is_empty() {
        return Vec::new();
    }
    let module_path = vec![Symbol::from("Schema")];
    let ddl = crate::emit::shared::schema_sql::render_schema_sql(schema);
    vec![LibraryFunction {
        module_path,
        name: Symbol::from("create_tables"),
        params: Vec::new(),
        body: lit_str(ddl),
        signature: Some(fn_sig(vec![], Ty::Str)),
        effects: EffectSet::default(),
    }]
}
