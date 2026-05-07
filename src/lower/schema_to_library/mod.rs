//! Lower the database `Schema` into a `Schema` LibraryFunction:
//! `Schema.statements -> Array<String>` returning the SQLite DDL as
//! a list of statements (one CREATE TABLE per table, one CREATE
//! INDEX per index). Each emitted target wraps the list in whatever
//! load-into-adapter idiom suits — TS iterates with `db.exec` per
//! statement; Spinel iterates with `adapter.execute_ddl`.
//!
//! Self-describing IR: each statement is materialized inside the
//! method body as a typed `Lit::Str`, so the walker emits each
//! string unchanged across every target. The lowerer relies on the
//! `emit::shared::schema_sql::render_schema_statements` for SQLite-
//! flavored DDL — keeps DDL rendering in one place; per-dialect
//! variants land here when other databases need support.
//!
//! Statements-list (rather than one joined string) is the general
//! shape: portable across DB drivers that don't accept multi-
//! statement input (Postgres' pg gem, MySQL drivers), and gives
//! per-statement error reporting in any adapter. The previous
//! single-string form was an accidentally TS-shaped choice
//! (better-sqlite3 happens to accept multi-statement); statements-
//! list matches what Spinel emit independently arrived at.

use crate::dialect::LibraryFunction;
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, ArrayStyle};
use crate::ident::Symbol;
use crate::lower::typing::{fn_sig, lit_str, with_ty};
use crate::schema::Schema;
use crate::span::Span;
use crate::ty::Ty;

/// Build the `Schema` module as a single LibraryFunction:
/// `Schema.statements() -> Array<String>` returning the rendered
/// DDL statements. Empty when `schema.tables` is empty — apps
/// without persisted models don't need a Schema artifact.
pub fn lower_schema_to_library_functions(schema: &Schema) -> Vec<LibraryFunction> {
    if schema.tables.is_empty() {
        return Vec::new();
    }
    let module_path = vec![Symbol::from("Schema")];
    let stmts = crate::emit::shared::schema_sql::render_schema_statements(schema);
    let elements: Vec<Expr> = stmts.into_iter().map(lit_str).collect();
    let body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array { elements, style: ArrayStyle::Brackets },
        ),
        Ty::Array { elem: Box::new(Ty::Str) },
    );
    vec![LibraryFunction {
        module_path,
        name: Symbol::from("statements"),
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Array { elem: Box::new(Ty::Str) })),
        effects: EffectSet::default(),
        is_async: false,
    }]
}
