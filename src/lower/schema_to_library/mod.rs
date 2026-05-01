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

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol};
use crate::lower::typing::{fn_sig, lit_str};
use crate::schema::Schema;
use crate::ty::Ty;

/// Build a `Schema` LibraryClass from `schema`. Module-shaped (no
/// inheritance), one `def self.create_tables` method body holding the
/// rendered DDL string. Returns `None` when `schema.tables` is empty
/// — apps without persisted models don't need a Schema artifact.
pub fn lower_schema_to_library_class(schema: &Schema) -> Option<LibraryClass> {
    if schema.tables.is_empty() {
        return None;
    }
    let owner = ClassId(Symbol::from("Schema"));
    let ddl = crate::emit::shared::schema_sql::render_schema_sql(schema);
    let body = lit_str(ddl);
    let method = MethodDef {
        name: Symbol::from("create_tables"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Str)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    };
    Some(LibraryClass {
        name: owner,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods: vec![method],
    })
}
