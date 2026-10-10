//! Exists probes (`_adapter_any?`, `_adapter_exists_by_id?`) — shared
//! `SELECT 1 … LIMIT 1` MethodDef shell so `adapter_emit` stays under 1k.

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol, TableRef};
use crate::lower::arel::{
    ArelOp, ArelVisitor, ColumnSpec, LimitSpec, Predicate, Select, SqliteVisitor,
};
use crate::schema::{Schema, Table};
use crate::ty::Ty;

use super::super::fn_sig;
use super::{eq_id_param, key_ty};

/// Shared `SELECT 1 … LIMIT 1` Exists MethodDef (`_adapter_any?` /
/// `_adapter_exists_by_id?`). Keeps both probes as one-liner wrappers so
/// this module does not grow a second copy of the Exists shell.
fn synth_exists_probe(
    owner: &ClassId,
    table: &Table,
    schema: &Schema,
    name: &str,
    conditions: Option<Predicate>,
    params: Vec<Param>,
    param_tys: Vec<(Symbol, Ty)>,
    param_binds: bool,
) -> MethodDef {
    let op = ArelOp::Select(Select {
        single_record: false,
        table: TableRef(table.name.clone()),
        columns: ColumnSpec::Exists,
        conditions,
        orders: vec![],
        limit: Some(LimitSpec(1)),
        joins: vec![],
        preloads: vec![],
    });
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from(name),
        receiver: MethodReceiver::Class,
        params,
        body: SqliteVisitor { param_binds }.visit(&op, schema, owner),
        signature: Some(fn_sig(param_tys, Ty::Bool)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// Unscoped emptiness without COUNT(*) — `Base.any?` / `none?` on
/// Level-3 models. Same Exists emit as `_adapter_exists_by_id?`, no WHERE.
pub(super) fn synth_adapter_any(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    synth_exists_probe(owner, table, schema, "_adapter_any?", None, vec![], vec![], false)
}

pub(super) fn synth_adapter_exists_by_id(owner: &ClassId, table: &Table, schema: &Schema, param_binds: bool) -> MethodDef {
    let id = Symbol::from("id");
    let key = key_ty(table);
    synth_exists_probe(
        owner,
        table,
        schema,
        "_adapter_exists_by_id?",
        Some(eq_id_param(table, &id)),
        vec![Param::positional(id.clone())],
        vec![(id, key)],
        param_binds,
    )
}

