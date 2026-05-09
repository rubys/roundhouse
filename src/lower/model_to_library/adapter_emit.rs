//! Per-model adapter primitives — Level-3 emit, built atop Arel IR.
//!
//! For each model with a known schema, synthesize per-model class methods
//! that go directly from SQL composition to typed model instances. Each
//! method's body is built by:
//!
//!   1. Constructing an `ArelOp` describing the operation.
//!   2. Calling `SqliteVisitor::visit(op, schema, owner)` to get the
//!      target-runtime `Expr` over the `Db` primitive surface.
//!   3. Wrapping in a `MethodDef` with the appropriate name + signature.
//!
//! The `Db` primitive surface (configure / prepare / step? / column_int /
//! column_text / finalize / exec / escape_*) is the runtime contract these
//! primitives sit on top of; the public AR API in
//! `runtime/ruby/active_record/base.rb` delegates to these primitives.
//! `Db` is backend-agnostic — sibling shims (cruby/sqlite-gem,
//! spinel-FFI/sqlite, postgres/etc.) implement the same module name;
//! per-database SQL dialect differences live in the visitor's per-backend
//! impl.
//!
//! Underscore-prefix on emitted names signals "framework-internal, not
//! user-facing API." See project_level_3_adapter_emit.md and
//! project_arel_compile_time_first.md.
//!
//! Methods emitted (uniform per-model shape — no app-scan needed):
//!   `_adapter_find_by_id(id)` — find by primary key, returns `<Owner>?`
//!   `_adapter_all` — full table scan, returns `Array[<Owner>]`
//!   `_adapter_insert(instance)` — INSERT, returns last_insert_rowid
//!   `_adapter_update(id, instance)` — UPDATE WHERE id, returns void
//!   `_adapter_delete(id)` — DELETE WHERE id, returns void
//!   `_adapter_count` — SELECT COUNT(*), returns Integer
//!   `_adapter_exists_by_id?(id)` — SELECT 1 LIMIT 1, returns Bool
//!   `_adapter_truncate` — DELETE FROM table (test setup)

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol, TableRef, VarId};
use crate::lower::arel::{
    ArelOp, ArelVisitor, Assignment, ColRef, ColumnSpec, Delete, Insert, LimitSpec, Predicate,
    Select, SqliteVisitor, Update, Value, ValueType,
};
use crate::schema::{Schema, Table};
use crate::span::Span;
use crate::ty::Ty;

use super::{fn_sig, ty_of_column};

pub(super) fn push_adapter_methods(
    methods: &mut Vec<MethodDef>,
    owner: &ClassId,
    table: &Table,
    schema: &Schema,
) {
    methods.push(synth_adapter_find_by_id(owner, table, schema));
    methods.push(synth_adapter_all(owner, table, schema));
    methods.push(synth_adapter_insert(owner, table, schema));
    methods.push(synth_adapter_update(owner, table, schema));
    methods.push(synth_adapter_delete(owner, table, schema));
    methods.push(synth_adapter_count(owner, table, schema));
    methods.push(synth_adapter_exists_by_id(owner, table, schema));
    methods.push(synth_adapter_truncate(owner, table, schema));
}

// ---------------------------------------------------------------------------
// Each synth function builds an ArelOp for its shape, calls the visitor,
// and wraps in a MethodDef. The visitor produces the same Expr today's
// hand-written synth functions produced. See arel/visitor.rs for the
// per-shape emit (single hydrate / multi hydrate / count / exists /
// insert / update / delete).
// ---------------------------------------------------------------------------

fn synth_adapter_find_by_id(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let id = Symbol::from("id");
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    let nilable_owner = Ty::Union { variants: vec![owner_ty, Ty::Nil] };

    let op = ArelOp::Select(Select {
        table: TableRef(table.name.clone()),
        columns: ColumnSpec::All,
        conditions: Some(eq_id_param(table, &id)),
        orders: vec![],
        limit: Some(LimitSpec(1)),
        joins: vec![],
    });

    MethodDef {
        name: Symbol::from("_adapter_find_by_id"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(id.clone())],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![(id, Ty::Int)], nilable_owner)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

fn synth_adapter_all(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };

    let op = ArelOp::Select(Select {
        table: TableRef(table.name.clone()),
        columns: ColumnSpec::All,
        conditions: None,
        orders: vec![],
        limit: None,
        joins: vec![],
    });

    MethodDef {
        name: Symbol::from("_adapter_all"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![], Ty::Array { elem: Box::new(owner_ty) })),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

fn synth_adapter_insert(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let instance = Symbol::from("instance");
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };

    let assignments: Vec<Assignment> = table
        .columns
        .iter()
        .filter(|c| !c.primary_key)
        .map(|c| Assignment {
            column: c.name.clone(),
            value: Value::Runtime {
                expr: instance_field(&instance, &c.name),
                ty: value_type_for_column(&c.col_type),
            },
        })
        .collect();

    let op = ArelOp::Insert(Insert {
        table: TableRef(table.name.clone()),
        assignments,
    });

    MethodDef {
        name: Symbol::from("_adapter_insert"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(instance.clone())],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![(instance, owner_ty)], Ty::Int)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

fn synth_adapter_update(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let id = Symbol::from("id");
    let instance = Symbol::from("instance");
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };

    let assignments: Vec<Assignment> = table
        .columns
        .iter()
        .filter(|c| !c.primary_key)
        .map(|c| Assignment {
            column: c.name.clone(),
            value: Value::Runtime {
                expr: instance_field(&instance, &c.name),
                ty: value_type_for_column(&c.col_type),
            },
        })
        .collect();

    let op = ArelOp::Update(Update {
        table: TableRef(table.name.clone()),
        assignments,
        conditions: Some(eq_id_param(table, &id)),
    });

    MethodDef {
        name: Symbol::from("_adapter_update"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(id.clone()), Param::positional(instance.clone())],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![(id, Ty::Int), (instance, owner_ty)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

fn synth_adapter_delete(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let id = Symbol::from("id");

    let op = ArelOp::Delete(Delete {
        table: TableRef(table.name.clone()),
        conditions: Some(eq_id_param(table, &id)),
    });

    MethodDef {
        name: Symbol::from("_adapter_delete"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(id.clone())],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![(id, Ty::Int)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

fn synth_adapter_count(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let op = ArelOp::Select(Select {
        table: TableRef(table.name.clone()),
        columns: ColumnSpec::Count,
        conditions: None,
        orders: vec![],
        limit: None,
        joins: vec![],
    });

    MethodDef {
        name: Symbol::from("_adapter_count"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![], Ty::Int)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

fn synth_adapter_exists_by_id(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let id = Symbol::from("id");

    let op = ArelOp::Select(Select {
        table: TableRef(table.name.clone()),
        columns: ColumnSpec::Exists,
        conditions: Some(eq_id_param(table, &id)),
        orders: vec![],
        limit: Some(LimitSpec(1)),
        joins: vec![],
    });

    MethodDef {
        name: Symbol::from("_adapter_exists_by_id?"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(id.clone())],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![(id, Ty::Int)], Ty::Bool)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

fn synth_adapter_truncate(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let op = ArelOp::Delete(Delete {
        table: TableRef(table.name.clone()),
        conditions: None,
    });

    MethodDef {
        name: Symbol::from("_adapter_truncate"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `Eq(<table>.id, Runtime(<id-param>, Int))` — the WHERE shape
/// shared by find_by_id / update / delete / exists_by_id?.
fn eq_id_param(table: &Table, id_param: &Symbol) -> Predicate {
    Predicate::Eq(
        ColRef { table: TableRef(table.name.clone()), column: Symbol::from("id") },
        Value::Runtime { expr: var_ref(id_param), ty: ValueType::Int },
    )
}

fn instance_field(instance: &Symbol, col_name: &Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(instance)),
            method: col_name.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

fn var_ref(name: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: name.clone() })
}

fn value_type_for_column(t: &crate::schema::ColumnType) -> ValueType {
    match ty_of_column(t) {
        Ty::Int => ValueType::Int,
        Ty::Bool => ValueType::Bool,
        _ => ValueType::Str,
    }
}
