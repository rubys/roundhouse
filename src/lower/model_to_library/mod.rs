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

use std::collections::HashMap;

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Model};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::schema::{ColumnType, Schema, Table};
use crate::span::Span;
use crate::ty::{Row, Ty};

use self::associations::{push_association_methods, push_dependent_destroy};
use self::broadcasts::push_broadcasts_methods;
use self::markers::{push_block_callback_methods, push_unknown_marker_methods};
use self::schema::push_schema_methods;
use self::validations::push_validate_method;

/// Bulk entry point: lower every model in `models` against `schema`,
/// sharing one class registry so cross-model dispatch (`Article` calling
/// `Comment.where(...)`) types correctly. Use this for whole-app emit;
/// the single-model entry point below is for tests/probes.
///
/// `extra_class_infos` lets callers register additional ClassInfo
/// entries (e.g. lowered view modules so `Views::Articles.article(...)`
/// dispatches type) — passed as flat `(ClassId, ClassInfo)` pairs;
/// callers that want both the full path and a last-segment alias
/// should insert both.
pub fn lower_models_to_library_classes(
    models: &[Model],
    schema: &Schema,
    extra_class_infos: Vec<(ClassId, crate::analyze::ClassInfo)>,
) -> Vec<LibraryClass> {
    let mut all_methods: Vec<(Vec<MethodDef>, ClassId, Option<&Table>, &Model)> = Vec::new();
    for model in models {
        let methods = build_methods(model, schema);
        let table = schema.tables.get(&model.table.0);
        all_methods.push((methods, model.name.clone(), table, model));
    }

    let mut classes: HashMap<ClassId, crate::analyze::ClassInfo> = HashMap::new();
    for (methods, name, table, model) in &all_methods {
        let info = build_class_info(model, methods, *table);
        classes.insert(name.clone(), info);
    }
    // Framework runtime stubs — referenced from broadcasts_to expansions
    // but not part of any model. Mirrors runtime/ruby/broadcasts.rb's
    // public surface (each takes a kwargs hash and returns Nil).
    classes.insert(ClassId(Symbol::from("Broadcasts")), broadcasts_class_info());
    // Caller-supplied entries (typically the lowered view modules,
    // registered under both their full ClassId and a last-segment
    // alias for the typer's last-segment Const lookup).
    for (id, info) in extra_class_infos {
        classes.insert(id, info);
    }

    let mut out = Vec::new();
    for (mut methods, _, table, model) in all_methods {
        for method in &mut methods {
            type_method_body(method, &classes, table);
        }
        out.push(LibraryClass {
            name: model.name.clone(),
            is_module: false,
            parent: model.parent.clone(),
            includes: Vec::new(),
            methods,
        });
    }
    out
}

/// Build a `ClassInfo` for a lowered LibraryClass — used to feed
/// view modules / runtime-class lowerings into the model lowerer's
/// shared registry. Each `MethodDef.signature` becomes an entry in
/// `class_methods` (for `MethodReceiver::Class`) or `instance_methods`
/// (for `MethodReceiver::Instance`).
pub fn class_info_from_library_class(lc: &LibraryClass) -> crate::analyze::ClassInfo {
    let mut info = crate::analyze::ClassInfo::default();
    for m in &lc.methods {
        if let Some(sig) = &m.signature {
            match m.receiver {
                MethodReceiver::Instance => {
                    info.instance_methods.insert(m.name.clone(), sig.clone());
                }
                MethodReceiver::Class => {
                    info.class_methods.insert(m.name.clone(), sig.clone());
                }
            }
        }
    }
    info
}

/// Single-model entry point: lower one `Model` (Rails-shape, with DSL
/// items in `body`) into a post-lowering `LibraryClass`. Builds a
/// class registry containing only this model — for whole-app emit
/// where cross-model dispatch needs typing, prefer
/// `lower_models_to_library_classes`.
///
/// `schema` supplies the column list for the model's table — needed for
/// the per-column accessors / `attributes` / `[]` / `[]=` / `update` /
/// `initialize` lowerings. Models whose table isn't in the schema (rare;
/// abstract or virtual) get only the non-schema-driven methods.
pub fn lower_model_to_library_class(model: &Model, schema: &Schema) -> LibraryClass {
    let mut methods = build_methods(model, schema);
    let table = schema.tables.get(&model.table.0);
    let class_info = build_class_info(model, &methods, table);
    let mut classes: HashMap<ClassId, crate::analyze::ClassInfo> = HashMap::new();
    classes.insert(model.name.clone(), class_info);
    for method in &mut methods {
        type_method_body(method, &classes, table);
    }
    LibraryClass {
        name: model.name.clone(),
        is_module: false,
        parent: model.parent.clone(),
        includes: Vec::new(),
        methods,
    }
}

/// Untyped-body method synthesis — shared by the single-model and
/// bulk entry points. Body-typing is the caller's responsibility (it
/// needs the cross-model registry).
fn build_methods(model: &Model, schema: &Schema) -> Vec<MethodDef> {
    let mut methods: Vec<MethodDef> = Vec::new();

    if let Some(table) = schema.tables.get(&model.table.0) {
        push_schema_methods(&mut methods, model, table);
    }

    push_validate_method(&mut methods, model);
    push_association_methods(&mut methods, model);
    push_dependent_destroy(&mut methods, model);
    push_unknown_marker_methods(&mut methods, model);
    push_broadcasts_methods(&mut methods, model);
    push_block_callback_methods(&mut methods, model);

    methods
}

/// Construct the `ClassInfo` for a lowered model: schema-derived
/// attribute row, plus instance/class method tables built from the
/// synthesized `MethodDef.signature`s and an ApplicationRecord
/// baseline (save / destroy / persisted? / errors / find / all /
/// where / count / exists? / find_by / destroy_all).
fn build_class_info(
    model: &Model,
    methods: &[MethodDef],
    table: Option<&Table>,
) -> crate::analyze::ClassInfo {
    let mut info = crate::analyze::ClassInfo::default();
    info.table = Some(model.table.clone());

    // Attributes row from schema columns.
    if let Some(t) = table {
        let mut row = Row::closed();
        for col in &t.columns {
            row.fields.insert(col.name.clone(), ty_of_column(&col.col_type));
        }
        info.attributes = row;
    }

    // Synthesized method signatures.
    for m in methods {
        if let Some(sig) = &m.signature {
            match m.receiver {
                MethodReceiver::Instance => {
                    info.instance_methods.insert(m.name.clone(), sig.clone());
                }
                MethodReceiver::Class => {
                    info.class_methods.insert(m.name.clone(), sig.clone());
                }
            }
        }
    }

    // ApplicationRecord baseline (subset of runtime/ruby/active_record/base.rb's
    // public API that synthesized model bodies actually call). Only insert
    // when not already overridden by the lowerer.
    let class_id = &model.name;
    let owner_ty = Ty::Class { id: class_id.clone(), args: vec![] };
    let owner_or_nil = Ty::Union {
        variants: vec![owner_ty.clone(), Ty::Nil],
    };
    let array_owner = Ty::Array { elem: Box::new(owner_ty.clone()) };
    let any_hash = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };

    // ApplicationRecord declares `id` and `id=` (the schema synthesizers
    // skip the id column because it's inherited from the base class).
    insert_default(&mut info.instance_methods, "id", fn_sig(vec![], Ty::Int));
    insert_default(
        &mut info.instance_methods,
        "id=",
        fn_sig(vec![(Symbol::from("value"), Ty::Int)], Ty::Int),
    );
    insert_default(&mut info.instance_methods, "save", fn_sig(vec![], Ty::Bool));
    insert_default(&mut info.instance_methods, "save!", fn_sig(vec![], Ty::Bool));
    insert_default(&mut info.instance_methods, "destroy", fn_sig(vec![], owner_ty.clone()));
    insert_default(&mut info.instance_methods, "destroyed?", fn_sig(vec![], Ty::Bool));
    insert_default(&mut info.instance_methods, "persisted?", fn_sig(vec![], Ty::Bool));
    insert_default(
        &mut info.instance_methods,
        "mark_persisted!",
        fn_sig(vec![], Ty::Nil),
    );
    insert_default(
        &mut info.instance_methods,
        "errors",
        fn_sig(vec![], Ty::Class { id: ClassId(Symbol::from("ErrorCollection")), args: vec![] }),
    );
    insert_default(&mut info.instance_methods, "valid?", fn_sig(vec![], Ty::Bool));

    // Validations mixin — instance helpers expected on every record.
    insert_default(
        &mut info.instance_methods,
        "validates_presence_of",
        fn_sig(vec![(Symbol::from("attr"), Ty::Sym), (Symbol::from("value"), Ty::Untyped)], Ty::Nil),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_absence_of",
        fn_sig(vec![(Symbol::from("attr"), Ty::Sym), (Symbol::from("value"), Ty::Untyped)], Ty::Nil),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_length_of",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("value"), Ty::Untyped),
                (Symbol::from("opts"), any_hash.clone()),
            ],
            Ty::Nil,
        ),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_format_of",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("value"), Ty::Untyped),
                (Symbol::from("opts"), any_hash.clone()),
            ],
            Ty::Nil,
        ),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_numericality_of",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("value"), Ty::Untyped),
                (Symbol::from("opts"), any_hash.clone()),
            ],
            Ty::Nil,
        ),
    );
    insert_default(
        &mut info.instance_methods,
        "validates_inclusion_of",
        fn_sig(
            vec![
                (Symbol::from("attr"), Ty::Sym),
                (Symbol::from("value"), Ty::Untyped),
                (Symbol::from("opts"), any_hash.clone()),
            ],
            Ty::Nil,
        ),
    );

    // Class-level finders / scopes.
    insert_default(
        &mut info.class_methods,
        "find",
        fn_sig(vec![(Symbol::from("id"), Ty::Int)], owner_ty.clone()),
    );
    insert_default(
        &mut info.class_methods,
        "find_by",
        fn_sig(vec![(Symbol::from("attrs"), any_hash.clone())], owner_or_nil),
    );
    insert_default(
        &mut info.class_methods,
        "all",
        fn_sig(vec![], array_owner.clone()),
    );
    insert_default(
        &mut info.class_methods,
        "where",
        fn_sig(vec![(Symbol::from("conditions"), any_hash.clone())], array_owner.clone()),
    );
    insert_default(
        &mut info.class_methods,
        "count",
        fn_sig(vec![], Ty::Int),
    );
    insert_default(
        &mut info.class_methods,
        "exists?",
        fn_sig(vec![(Symbol::from("id"), Ty::Int)], Ty::Bool),
    );
    insert_default(
        &mut info.class_methods,
        "destroy_all",
        fn_sig(vec![], Ty::Int),
    );
    insert_default(
        &mut info.class_methods,
        "new",
        fn_sig(vec![(Symbol::from("attrs"), any_hash)], owner_ty),
    );

    info
}

fn insert_default(map: &mut HashMap<Symbol, Ty>, name: &str, sig: Ty) {
    map.entry(Symbol::from(name)).or_insert(sig);
}

/// Stub `ClassInfo` for the `Broadcasts` framework module. Each
/// helper takes a kwargs hash and returns Nil (per the Ruby
/// runtime). Only carrying signatures the model lowerer's
/// broadcasts expansion + block-form callbacks actually emit.
fn broadcasts_class_info() -> crate::analyze::ClassInfo {
    let mut info = crate::analyze::ClassInfo::default();
    let kwargs = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let sig = fn_sig(vec![(Symbol::from("opts"), kwargs)], Ty::Nil);
    info.class_methods.insert(Symbol::from("prepend"), sig.clone());
    info.class_methods.insert(Symbol::from("replace"), sig.clone());
    info.class_methods.insert(Symbol::from("remove"), sig.clone());
    info.class_methods.insert(Symbol::from("append"), sig);
    info
}

fn type_method_body(
    method: &mut MethodDef,
    classes: &HashMap<ClassId, crate::analyze::ClassInfo>,
    table: Option<&Table>,
) {
    let typer = crate::analyze::BodyTyper::new(classes);
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
    // Seed ivar_bindings from schema columns so bare `@title` reads
    // resolve to the column type. Same source the synthesizers used
    // for the fields themselves.
    if matches!(method.receiver, MethodReceiver::Instance) {
        if let Some(t) = table {
            for col in &t.columns {
                ctx.ivar_bindings
                    .insert(col.name.clone(), ty_of_column(&col.col_type));
            }
        }
    }
    // Opt-in to `recv: Some(SelfRef)` rewriting on bare Sends —
    // matches the pattern view_to_library uses.
    ctx.annotate_self_dispatch = true;
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
