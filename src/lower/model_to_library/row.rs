//! Per-model `<Model>Row` LibraryClass synthesis.
//!
//! For each model with a known schema, we synthesize a sibling class
//! whose only job is to hold the typed shape of an adapter row. The
//! adapter still returns `Hash[Symbol, untyped]` (one stable shim API
//! across targets), but the moment it crosses into framework Ruby the
//! Hash is widened to a typed object via `<Model>Row.from_raw(hash)`.
//! Every downstream call site sees typed slots.
//!
//! Concretely, for an `Article` model with columns
//! `[id, title, body, created_at, updated_at]`:
//!
//! ```ruby
//! class ArticleRow
//!   attr_accessor :id, :title, :body, :created_at, :updated_at
//!
//!   def self.from_raw(row)
//!     instance = new
//!     instance.id         = row[:id] || 0
//!     instance.title      = row[:title]
//!     instance.body       = row[:body]
//!     instance.created_at = row[:created_at]
//!     instance.updated_at = row[:updated_at]
//!     instance
//!   end
//! end
//! ```
//!
//! Tagged with `LibraryClassOrigin::ResourceRow { resource, fields }`
//! so per-target emitters can group / collapse if their target benefits
//! (per `project_specialization_strategy.md`).

use crate::dialect::{
    AccessorKind, LibraryClass, LibraryClassOrigin, MethodDef, MethodReceiver, Param,
};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol, VarId};
use crate::schema::{Column, Schema, Table};
use crate::span::Span;
use crate::ty::Ty;

use super::{
    class_const, fn_sig, is_id_column, lit_int, lit_sym, seq, ty_of_column, var_ref, with_ty,
};

/// Synthesize the per-model `<Model>Row` LibraryClasses for every model
/// whose table is in `schema`. Models without a known table get no Row
/// (rare; abstract / virtual models). One Row class per model — fields
/// come from the schema column list in declaration order.
pub fn synthesize_row_classes(
    models: &[crate::dialect::Model],
    schema: &Schema,
) -> Vec<LibraryClass> {
    let mut out = Vec::with_capacity(models.len());
    for model in models {
        let Some(table) = schema.tables.get(&model.table.0) else {
            continue;
        };
        out.push(build_row_class(&model.name, table));
    }
    out
}

fn build_row_class(model_name: &ClassId, table: &Table) -> LibraryClass {
    let row_class_id = row_class_id(model_name);
    let mut methods: Vec<MethodDef> = Vec::new();

    // Per-column reader/writer (attr_accessor at source level, expanded
    // to method pairs at IR level so the universal post-lowering shape
    // applies — same convention the model lowerer's schema synth uses).
    for col in &table.columns {
        methods.push(synth_row_attr_reader(&row_class_id, col));
        methods.push(synth_row_attr_writer(&row_class_id, col));
    }

    methods.push(synth_row_from_raw(&row_class_id, table));

    let fields: Vec<Symbol> = table.columns.iter().map(|c| c.name.clone()).collect();
    LibraryClass {
        name: row_class_id,
        is_module: false,
        parent: None,
        includes: Vec::new(),
        methods,
        origin: Some(LibraryClassOrigin::ResourceRow {
            resource: resource_sym(model_name),
            fields,
        }),
    }
}

/// `<Model>Row` ClassId — the synthesized row holder's name. Always
/// the model's class name suffixed with `Row` (e.g. Article → ArticleRow).
pub fn row_class_id(model_name: &ClassId) -> ClassId {
    ClassId(Symbol::from(format!("{}Row", model_name.0.as_str())))
}

/// Lowercase symbol form of the model name (e.g. Article → :article).
/// Used as the `resource` tag in the origin.
fn resource_sym(model_name: &ClassId) -> Symbol {
    Symbol::from(crate::naming::snake_case(model_name.0.as_str()))
}

fn synth_row_attr_reader(owner: &ClassId, col: &Column) -> MethodDef {
    let col_ty = ty_of_column(&col.col_type);
    let body = with_ty(
        Expr::new(Span::synthetic(), ExprNode::Ivar { name: col.name.clone() }),
        col_ty.clone(),
    );
    MethodDef {
        name: col.name.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], col_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeReader,
    }
}

fn synth_row_attr_writer(owner: &ClassId, col: &Column) -> MethodDef {
    let value_param = Symbol::from("value");
    let col_ty = ty_of_column(&col.col_type);
    let rhs = with_ty(var_ref(value_param.clone()), col_ty.clone());
    let body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: col.name.clone() },
                value: rhs,
            },
        ),
        col_ty.clone(),
    );
    MethodDef {
        name: Symbol::from(format!("{}=", col.name.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value_param.clone())],
        body,
        signature: Some(fn_sig(vec![(value_param, col_ty.clone())], col_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeWriter,
    }
}

/// `def self.from_raw(row)` — the boundary where the adapter's
/// `Hash[Symbol, untyped]` widens once into typed slots. Subsequent
/// uses see `<Model>Row` directly; no `sp_RbVal` / `Record<string, any>`
/// flowing through the model layer.
fn synth_row_from_raw(owner: &ClassId, table: &Table) -> MethodDef {
    let row = Symbol::from("row");
    let instance = Symbol::from("instance");

    let new_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("new"),
            args: Vec::new(),
            block: None,
            parenthesized: true,
        },
    );

    let mut stmts: Vec<Expr> = Vec::new();
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: instance.clone() },
            value: new_call,
        },
    ));

    for col in &table.columns {
        let col_ty = ty_of_column(&col.col_type);
        let lookup = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(row.clone())),
                method: Symbol::from("[]"),
                args: vec![lit_sym(col.name.clone())],
                block: None,
                parenthesized: false,
            },
        );
        // ID-shaped columns get `|| 0` defaults — same semantics as
        // the model's existing initialize: missing-id maps to "unsaved"
        // sentinel (0), not nil. Keeps integer slots integer.
        let raw_value = if is_id_column(&col.name) {
            Expr::new(
                Span::synthetic(),
                ExprNode::BoolOp {
                    op: crate::expr::BoolOpKind::Or,
                    surface: crate::expr::BoolOpSurface::Symbol,
                    left: lookup,
                    right: lit_int(0),
                },
            )
        } else {
            lookup
        };
        // Wrap each adapter-row value in a `Cast` IR node so strict-
        // typed targets (Crystal `.as(T)`, future Rust `try_into`)
        // bridge the adapter's wide row-value type (DB::Any union /
        // sp_RbVal / sqlx Row::get<T>) into the column's declared
        // type. Ruby/Spinel emit unwraps Cast as the inner value (no
        // cast operator needed); TS emit either no-ops or emits
        // `(value as T)` depending on width.
        let value = Expr::new(
            Span::synthetic(),
            ExprNode::Cast {
                value: raw_value,
                target_ty: col_ty,
            },
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from(format!("{}=", col.name.as_str())),
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        ));
    }

    stmts.push(var_ref(instance));

    let row_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    MethodDef {
        name: Symbol::from("from_raw"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(row.clone())],
        body: seq(stmts),
        signature: Some(fn_sig(vec![(row, row_ty)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

/// Build the `ClassInfo` registry entry for a synthesized Row class.
/// Mirrors what `model_to_library`'s `build_class_info` does for models,
/// but trimmed to what a Row class actually has: per-column attr +
/// `from_raw`. No ApplicationRecord baseline — Row is a plain holder.
pub fn row_class_info(lc: &LibraryClass) -> crate::analyze::ClassInfo {
    let mut info = crate::analyze::ClassInfo::default();
    for m in &lc.methods {
        if let Some(sig) = &m.signature {
            match m.receiver {
                MethodReceiver::Instance => {
                    info.instance_methods.insert(m.name.clone(), sig.clone());
                    info.instance_method_kinds.insert(m.name.clone(), m.kind);
                }
                MethodReceiver::Class => {
                    info.class_methods.insert(m.name.clone(), sig.clone());
                    info.class_method_kinds.insert(m.name.clone(), m.kind);
                }
            }
        }
    }
    info
}
