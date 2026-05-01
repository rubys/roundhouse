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
use crate::schema::{ColumnType, Schema};
use crate::span::Span;
use crate::ty::Ty;

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

    // Run the body-typer over each lowered method so leaf nodes
    // (Var, Ivar, SelfRef, Const) and composites (Send, Assign, Seq,
    // BoolOp, Case) pick up types the synthesizers can't conveniently
    // populate inline. Seeded by the signature populated above (params
    // get types from sig_params; self_ty from enclosing_class). Mirrors
    // `view_to_library::type_method_body`.
    for method in &mut methods {
        type_method_body(method);
    }

    LibraryClass {
        name: model.name.clone(),
        is_module: false,
        parent: model.parent.clone(),
        includes: Vec::new(),
        methods,
    }
}

fn type_method_body(method: &mut MethodDef) {
    let empty_classes: std::collections::HashMap<
        crate::ident::ClassId,
        crate::analyze::ClassInfo,
    > = std::collections::HashMap::new();
    let typer = crate::analyze::BodyTyper::new(&empty_classes);
    let mut ctx = crate::analyze::Ctx::default();
    if let Some(Ty::Fn { params, .. }) = &method.signature {
        for (param, sig) in method.params.iter().zip(params.iter()) {
            ctx.local_bindings.insert(param.name.clone(), sig.ty.clone());
        }
    }
    if let Some(enclosing) = &method.enclosing_class {
        ctx.self_ty = Some(Ty::Class {
            id: ClassId(enclosing.clone()),
            args: vec![],
        });
    }
    typer.analyze_expr(&mut method.body, &ctx);
}

// ---------------------------------------------------------------------------
// Small ExprNode constructors used throughout. Each takes a synthetic span
// since lowered methods don't correspond to a single source location.
// ---------------------------------------------------------------------------

pub(super) fn lit_str(s: String) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s } }),
        Ty::Str,
    )
}

pub(super) fn lit_sym(name: Symbol) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: name } }),
        Ty::Sym,
    )
}

pub(super) fn lit_int(value: i64) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value } }),
        Ty::Int,
    )
}

pub(super) fn lit_float(value: f64) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value } }),
        Ty::Float,
    )
}

pub(super) fn nil_lit() -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        Ty::Nil,
    )
}

/// Attach a known type to an Expr. Lowerers use this when the type is
/// statically known by construction — avoiding a separate analyzer
/// pass to rediscover what we already knew.
pub(super) fn with_ty(mut e: Expr, ty: Ty) -> Expr {
    e.ty = Some(ty);
    e
}

/// Schema column type → roundhouse `Ty`. Mirrors `ingest::model::ty_of_column`
/// — duplicated here to avoid making that internal helper public for one
/// caller. Keep them in sync; the mapping is small and stable.
pub(super) fn ty_of_column(t: &ColumnType) -> Ty {
    match t {
        ColumnType::Integer | ColumnType::BigInt => Ty::Int,
        ColumnType::Float | ColumnType::Decimal { .. } => Ty::Float,
        ColumnType::String { .. } | ColumnType::Text => Ty::Str,
        ColumnType::Boolean => Ty::Bool,
        ColumnType::Date | ColumnType::DateTime | ColumnType::Time => {
            Ty::Class { id: ClassId(Symbol::from("Time")), args: vec![] }
        }
        ColumnType::Binary => Ty::Str,
        ColumnType::Json => Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) },
        ColumnType::Reference { .. } => Ty::Int,
    }
}

/// Build a `Ty::Fn` signature from positional (name, type) pairs and a return type.
/// Effects default to pure — callers refine if needed (lifecycle hooks etc.).
pub(super) fn fn_sig(params: Vec<(Symbol, Ty)>, ret: Ty) -> Ty {
    Ty::Fn {
        params: params
            .into_iter()
            .map(|(name, ty)| crate::ty::Param {
                name,
                ty,
                kind: crate::ty::ParamKind::Required,
            })
            .collect(),
        block: None,
        ret: Box::new(ret),
        effects: crate::effect::EffectSet::pure(),
    }
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
