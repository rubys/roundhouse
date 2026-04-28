//! Lower a Rails-shape `Model` (with associations, validations, callbacks,
//! Schema-derived columns) into a post-lowering `LibraryClass` whose body
//! is a flat sequence of `MethodDef`s — the universal IR shape every
//! emitter consumes (see `project_universal_post_lowering_ir.md`).
//!
//! The output target is `fixtures/spinel-blog/app/models/<model>.rb`:
//! explicit method bodies (`def title; @title; end`, `def comments;
//! Comment.where(article_id: @id); end`, `def validate;
//! validates_presence_of(:title) { @title }; end`), no Rails DSL.
//!
//! This module is pure: input is one `Model` plus the app `Schema`, output
//! is one `LibraryClass`. No side-effects, no per-target choices. Per-Rails-
//! idiom lowering is a separate function so each can be tested in
//! isolation (skeleton, schema columns, has_many, belongs_to, validates,
//! callbacks, …).
//!
//! Strangler-fig direction: this lives alongside the existing per-target
//! emit paths. Callers that consume the post-lowering shape opt in
//! explicitly; the rich `Model` dialect remains the input for emitters
//! that haven't migrated.

mod schema;
mod validations;
mod associations;
mod broadcasts;
mod markers;

use crate::dialect::{LibraryClass, MethodDef, Model};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::schema::Schema;
use crate::span::Span;

use self::associations::{push_association_methods, push_dependent_destroy};
use self::broadcasts::push_broadcasts_methods;
use self::markers::{push_block_callback_methods, push_unknown_marker_methods};
use self::schema::push_schema_methods;
use self::validations::push_validate_method;

/// Entry point: take a `Model` (Rails-shape, with DSL items in `body`) and
/// produce the post-lowering `LibraryClass` whose `methods` carry every
/// Rails idiom expanded into explicit method bodies.
///
/// `schema` supplies the column list for the model's table — needed for
/// the per-column accessors / `attributes` / `[]` / `[]=` / `update` /
/// `initialize` lowerings. Models whose table isn't in the schema (rare;
/// abstract or virtual) get only the non-schema-driven methods.
pub fn lower_model_to_library_class(model: &Model, schema: &Schema) -> LibraryClass {
    let mut methods: Vec<MethodDef> = Vec::new();

    if let Some(table) = schema.tables.get(&model.table.0) {
        push_schema_methods(&mut methods, model, table);
    }

    push_validate_method(&mut methods, model);
    push_association_methods(&mut methods, model);
    push_dependent_destroy(&mut methods, model);
    push_unknown_marker_methods(&mut methods, model);
    // broadcasts_to expansion runs BEFORE block-form callbacks so the
    // expansion's emitted statements appear first in the composed
    // method body — matches spinel-blog's source order, where the
    // broadcasts_to-derived call leads and the explicit block-form
    // cascade follows.
    push_broadcasts_methods(&mut methods, model);
    push_block_callback_methods(&mut methods, model);

    LibraryClass {
        name: model.name.clone(),
        is_module: false,
        parent: model.parent.clone(),
        includes: Vec::new(),
        methods,
    }
}

// ---------------------------------------------------------------------------
// Small ExprNode constructors used throughout. Each takes a synthetic span
// since lowered methods don't correspond to a single source location.
// ---------------------------------------------------------------------------

pub(super) fn lit_str(s: String) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s } })
}

pub(super) fn lit_sym(name: Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: name } })
}

pub(super) fn lit_int(value: i64) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value } })
}

pub(super) fn lit_float(value: f64) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value } })
}

pub(super) fn nil_lit() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
}

pub(super) fn var_ref(name: Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name })
}

pub(super) fn class_const(id: &ClassId) -> Expr {
    let path: Vec<Symbol> = id.0.as_str().split("::").map(Symbol::from).collect();
    Expr::new(Span::synthetic(), ExprNode::Const { path })
}

pub(super) fn self_ref() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::SelfRef)
}

pub(super) fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}

pub(super) fn is_id_column(name: &Symbol) -> bool {
    let s = name.as_str();
    s == "id" || s.ends_with("_id")
}
