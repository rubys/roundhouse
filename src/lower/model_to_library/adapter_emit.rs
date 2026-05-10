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
    methods.push(synth_adapter_reload(owner, table));
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

/// `def _adapter_insert` — instance method; reads ivars to compose
/// the INSERT, returns last_insert_rowid. Instance-method (not
/// class-method) so save() reaches it via implicit-self dispatch
/// (`@id = _adapter_insert`) and the TS emitter places the libsql
/// `await` on the call result rather than the receiver. See the
/// reload comment for the underlying emit issue with
/// `self.class.<async_method>`.
fn synth_adapter_insert(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let assignments: Vec<Assignment> = table
        .columns
        .iter()
        .filter(|c| !c.primary_key)
        .map(|c| Assignment {
            column: c.name.clone(),
            value: Value::Runtime {
                expr: ivar_ref(&c.name),
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
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![], Ty::Int)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

/// `def _adapter_update` — instance method; reads ivars + @id.
/// See `synth_adapter_insert` for the receiver-rationale.
fn synth_adapter_update(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let assignments: Vec<Assignment> = table
        .columns
        .iter()
        .filter(|c| !c.primary_key)
        .map(|c| Assignment {
            column: c.name.clone(),
            value: Value::Runtime {
                expr: ivar_ref(&c.name),
                ty: value_type_for_column(&c.col_type),
            },
        })
        .collect();

    let op = ArelOp::Update(Update {
        table: TableRef(table.name.clone()),
        assignments,
        conditions: Some(eq_id_ivar(table)),
    });

    MethodDef {
        name: Symbol::from("_adapter_update"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

/// `def _adapter_delete` — instance method; reads @id.
/// See `synth_adapter_insert` for the receiver-rationale.
fn synth_adapter_delete(owner: &ClassId, table: &Table, schema: &Schema) -> MethodDef {
    let op = ArelOp::Delete(Delete {
        table: TableRef(table.name.clone()),
        conditions: Some(eq_id_ivar(table)),
    });

    MethodDef {
        name: Symbol::from("_adapter_delete"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: SqliteVisitor.visit(&op, schema, owner),
        signature: Some(fn_sig(vec![], Ty::Nil)),
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

/// `def _adapter_reload` — SELECT-and-assign-into-self variant of
/// `_adapter_find_by_id`. Re-reads the row by `@id`, writes columns
/// back into `self` (preserving identity); returns self when the row
/// is still present, nil when it has been deleted. Backs framework
/// Ruby's `Base#reload`.
///
/// Modelled as an INSTANCE method (not a class method) so callers
/// reach it via implicit-self dispatch (`_adapter_reload`) rather
/// than `self.class._adapter_reload(self)`. The class-method form
/// trips an emit issue under the libsql async profile where the
/// emitter mishandles `self.class.<async_method>` — it lifts the
/// await to the receiver Send (`(await this.constructor).…`) and
/// drops the Promise from the actual call.
///
/// Built inline (not through the visitor) because the visitor's
/// single-hydrate shape always constructs a fresh `Owner.new`;
/// reload needs to write into self. Generalizing the visitor with
/// a "hydrate target = bare ivar / passed-in symbol" option is the
/// right cleanup once a second use surfaces.
fn synth_adapter_reload(owner: &ClassId, table: &Table) -> MethodDef {
    use crate::expr::{ExprNode, LValue, Literal};
    use crate::span::Span;

    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let db = ClassId(Symbol::from("Db"));
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    let nilable_owner = Ty::Union { variants: vec![owner_ty.clone(), Ty::Nil] };

    // SQL: "SELECT <cols> FROM <table> WHERE id = " + Db.escape_int(@id) + " LIMIT 1"
    let cols_csv: String = table
        .columns
        .iter()
        .map(|c| c.name.as_str().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let sql_prefix = arel_lit_str(format!(
        "SELECT {} FROM {} WHERE id = ",
        cols_csv,
        table.name.as_str()
    ));
    let id_ivar = Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from("id") });
    let escape_id = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("Db")] },
            )),
            method: Symbol::from("escape_int"),
            args: vec![id_ivar],
            block: None,
            parenthesized: true,
        },
    );
    let sql_suffix = arel_lit_str(" LIMIT 1".to_string());
    let sql_concat = arel_concat(vec![sql_prefix, escape_id, sql_suffix]);

    // stmt = Db.prepare(sql)
    let stmt_assign = arel_assign(
        &stmt,
        arel_db_call(&db, "prepare", vec![sql_concat]),
    );

    // result = nil
    let result_init = arel_assign(
        &result,
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
    );

    // if Db.step?(stmt) ; @<col> = Db.column_<int|text>(stmt, i) ; ... ; result = self ; end
    let mut if_body: Vec<Expr> = Vec::new();
    for (i, col) in table.columns.iter().enumerate() {
        let read_method = match ty_of_column(&col.col_type) {
            Ty::Int => "column_int",
            _ => "column_text",
        };
        let read_call = arel_db_call(
            &db,
            read_method,
            vec![var_ref(&stmt), arel_lit_int(i as i64)],
        );
        if_body.push(Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: col.name.clone() },
                value: read_call,
            },
        ));
    }
    if_body.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
            method: Symbol::from("mark_persisted!"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    ));
    if_body.push(arel_assign(
        &result,
        Expr::new(Span::synthetic(), ExprNode::SelfRef),
    ));

    let if_expr = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: arel_db_call(&db, "step?", vec![var_ref(&stmt)]),
            then_branch: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: if_body }),
            else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        },
    );

    let finalize = arel_db_call(&db, "finalize", vec![var_ref(&stmt)]);
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Seq {
            exprs: vec![stmt_assign, result_init, if_expr, finalize, var_ref(&result)],
        },
    );

    MethodDef {
        name: Symbol::from("_adapter_reload"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body,
        signature: Some(fn_sig(vec![], nilable_owner)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

// Inline expr helpers used by synth_adapter_reload — the ones in
// `super` are pub(super) but require helper visibility we don't want
// to widen for one synth function. Naming-prefixed to avoid shadowing.
fn arel_lit_str(s: String) -> Expr {
    use crate::expr::{ExprNode, Literal};
    use crate::span::Span;
    let mut e = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s } });
    e.ty = Some(Ty::Str);
    e
}

fn arel_lit_int(value: i64) -> Expr {
    use crate::expr::{ExprNode, Literal};
    use crate::span::Span;
    let mut e = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value } });
    e.ty = Some(Ty::Int);
    e
}

fn arel_assign(name: &Symbol, value: Expr) -> Expr {
    use crate::expr::{ExprNode, LValue};
    use crate::ident::VarId;
    use crate::span::Span;
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: name.clone() },
            value,
        },
    )
}

fn arel_db_call(db: &ClassId, method: &str, args: Vec<Expr>) -> Expr {
    use crate::expr::ExprNode;
    use crate::span::Span;
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::Const {
                path: db.0.as_str().split("::").map(Symbol::from).collect(),
            })),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn arel_concat(segments: Vec<Expr>) -> Expr {
    use crate::expr::ExprNode;
    use crate::span::Span;
    let mut iter = segments.into_iter();
    let mut acc = iter.next().expect("arel_concat needs at least one segment");
    for next in iter {
        acc = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(acc),
                method: Symbol::from("+"),
                args: vec![next],
                block: None,
                parenthesized: false,
            },
        );
    }
    acc
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `Eq(<table>.id, Runtime(<id-param>, Int))` — find_by_id /
/// exists_by_id? shape (id arrives as a method param).
fn eq_id_param(table: &Table, id_param: &Symbol) -> Predicate {
    Predicate::Eq(
        ColRef { table: TableRef(table.name.clone()), column: Symbol::from("id") },
        Value::Runtime { expr: var_ref(id_param), ty: ValueType::Int },
    )
}

/// `Eq(<table>.id, Runtime(@id, Int))` — instance-method update /
/// delete shape (id is read from the instance ivar). Used so save /
/// destroy can dispatch to `_adapter_update` / `_adapter_delete` via
/// implicit-self (`_adapter_update`) instead of the
/// `self.class._adapter_update(@id, self)` chain that the TS emitter
/// mishandles under the libsql async profile.
fn eq_id_ivar(table: &Table) -> Predicate {
    Predicate::Eq(
        ColRef { table: TableRef(table.name.clone()), column: Symbol::from("id") },
        Value::Runtime { expr: ivar_ref(&Symbol::from("id")), ty: ValueType::Int },
    )
}

fn ivar_ref(name: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() })
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
