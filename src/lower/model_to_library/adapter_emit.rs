//! Per-model adapter primitives — Level-3 emit.
//!
//! For each model with a known schema, synthesize per-model class methods
//! that go directly from SQL composition to typed model instances. The
//! `Db` primitive surface (configure / prepare / step? / column_int /
//! column_text / finalize / exec) is the runtime contract these primitives
//! sit on top of; the public AR API in `runtime/ruby/active_record/base.rb`
//! delegates to these primitives. `Db` is backend-agnostic — sibling
//! shims (cruby/sqlite-gem, spinel-FFI/sqlite, postgres/etc.) implement
//! the same module name; per-database SQL dialect differences live in
//! a separate dialect helper consulted at SQL composition time.
//!
//! Underscore-prefix on emitted names signals "framework-internal, not
//! user-facing API." See project_level_3_adapter_emit.md.
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
//!
//! Per-shape methods (app-scan needed): `_adapter_where_by_<cols>`,
//! `_adapter_find_by_<cols>` — deferred to a subsequent slice.

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol, VarId};
use crate::schema::Table;
use crate::span::Span;
use crate::ty::Ty;

use super::{class_const, fn_sig, lit_int, lit_str, nil_lit, seq, ty_of_column, var_ref};

const DB_MOD: &str = "Db";

pub(super) fn push_adapter_methods(
    methods: &mut Vec<MethodDef>,
    owner: &ClassId,
    table: &Table,
) {
    methods.push(synth_adapter_find_by_id(owner, table));
    methods.push(synth_adapter_all(owner, table));
    methods.push(synth_adapter_insert(owner, table));
    methods.push(synth_adapter_update(owner, table));
    methods.push(synth_adapter_delete(owner, table));
    methods.push(synth_adapter_count(owner, table));
    methods.push(synth_adapter_exists_by_id(owner, table));
    methods.push(synth_adapter_truncate(owner, table));
}

// Helper: build the SELECT-and-hydrate sequence body. `cond_clause`
// is appended verbatim to the SELECT (e.g. "WHERE id = " + id.to_s,
// or empty for full table scan). Returns Vec of statements that
// hydrate `instance` from a stepped stmt — caller embeds this in
// the right control-flow shape (single-row or loop).
fn hydrate_instance_body(owner: &ClassId, table: &Table, db: &ClassId, stmt: &Symbol, instance: &Symbol) -> Vec<Expr> {
    let mut stmts = Vec::new();
    let new_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("new"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    );
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: instance.clone() },
            value: new_call,
        },
    ));
    for (i, col) in table.columns.iter().enumerate() {
        let col_ty = ty_of_column(&col.col_type);
        let read_method = if matches!(col_ty, Ty::Int) { "column_int" } else { "column_text" };
        let read_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(class_const(db)),
                method: Symbol::from(read_method),
                args: vec![var_ref(stmt.clone()), lit_int(i as i64)],
                block: None,
                parenthesized: true,
            },
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from(format!("{}=", col.name.as_str())),
                args: vec![read_call],
                block: None,
                parenthesized: false,
            },
        ));
    }
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(instance.clone())),
            method: Symbol::from("mark_persisted!"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    ));
    stmts
}

// Helper: SELECT column list as a CSV (matches schema column order).
fn select_cols_csv(table: &Table) -> String {
    table.columns.iter().map(|c| c.name.as_str().to_string()).collect::<Vec<_>>().join(", ")
}

// Helper: Db.<method>(args) Send.
fn db_call(db: &ClassId, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(db)),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

/// `def self._adapter_find_by_id(id)`
///
/// ```ruby
/// def self._adapter_find_by_id(id)
///   stmt = Db.prepare("SELECT <cols> FROM <table> WHERE id = " + id.to_s + " LIMIT 1")
///   result = nil
///   if Db.step?(stmt)
///     instance = new
///     instance.<col0> = Db.column_<int|text>(stmt, 0)
///     ...
///     instance.mark_persisted!
///     result = instance
///   end
///   Db.finalize(stmt)
///   result
/// end
/// ```
fn synth_adapter_find_by_id(owner: &ClassId, table: &Table) -> MethodDef {
    let id = Symbol::from("id");
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let instance = Symbol::from("instance");
    let db = ClassId(Symbol::from(DB_MOD));

    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    let nilable_owner = Ty::Union { variants: vec![owner_ty.clone(), Ty::Nil] };

    // SQL: "SELECT <c0>, <c1>, ... FROM <table> WHERE id = " + id.to_s + " LIMIT 1"
    //
    // Inline the id via string concat rather than bind params: spinel's FFI
    // MVP can't construct the SQLITE_TRANSIENT destructor sentinel for
    // bind_text, so the FFI shim will inline values anyway. id.to_s is
    // safe — the param is typed Integer.
    let cols_csv = table
        .columns
        .iter()
        .map(|c| c.name.as_str().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let select_prefix = format!("SELECT {} FROM {} WHERE id = ", cols_csv, table.name.as_str());
    let select_suffix = " LIMIT 1".to_string();

    let id_to_s = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(id.clone())),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let sql_expr = bin_concat(bin_concat(lit_str(select_prefix), id_to_s), lit_str(select_suffix));

    // stmt = Db.prepare(sql)
    let prepare_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(&db)),
            method: Symbol::from("prepare"),
            args: vec![sql_expr],
            block: None,
            parenthesized: true,
        },
    );
    let stmt_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: stmt.clone() },
            value: prepare_call,
        },
    );

    // result = nil
    let result_init = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: result.clone() },
            value: nil_lit(),
        },
    );

    // if Db.step?(stmt) ... end
    let step_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(&db)),
            method: Symbol::from("step?"),
            args: vec![var_ref(stmt.clone())],
            block: None,
            parenthesized: true,
        },
    );

    // instance = new
    let new_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("new"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    );

    let mut if_body: Vec<Expr> = Vec::new();
    if_body.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: instance.clone() },
            value: new_call,
        },
    ));

    // Per-column reads. Pick column_int vs column_text based on the
    // schema-derived column type. Integer columns (id, FKs, ints) →
    // column_int; everything else → column_text. Future column types
    // (Float, Bool, etc.) will pick up additional Db primitives.
    for (i, col) in table.columns.iter().enumerate() {
        let col_ty = ty_of_column(&col.col_type);
        let read_method = if matches!(col_ty, Ty::Int) {
            "column_int"
        } else {
            "column_text"
        };
        let read_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(class_const(&db)),
                method: Symbol::from(read_method),
                args: vec![var_ref(stmt.clone()), lit_int(i as i64)],
                block: None,
                parenthesized: true,
            },
        );
        if_body.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var_ref(instance.clone())),
                method: Symbol::from(format!("{}=", col.name.as_str())),
                args: vec![read_call],
                block: None,
                parenthesized: false,
            },
        ));
    }

    // instance.mark_persisted!
    if_body.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(instance.clone())),
            method: Symbol::from("mark_persisted!"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    ));
    // result = instance
    if_body.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: result.clone() },
            value: var_ref(instance),
        },
    ));

    let if_expr = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: step_call,
            then_branch: seq(if_body),
            else_branch: nil_lit(),
        },
    );

    // Db.finalize(stmt)
    let finalize_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(&db)),
            method: Symbol::from("finalize"),
            args: vec![var_ref(stmt)],
            block: None,
            parenthesized: true,
        },
    );

    let body = seq(vec![stmt_assign, result_init, if_expr, finalize_call, var_ref(result)]);

    MethodDef {
        name: Symbol::from("_adapter_find_by_id"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(id.clone())],
        body,
        signature: Some(fn_sig(vec![(id, Ty::Int)], nilable_owner)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

fn bin_concat(left: Expr, right: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(left),
            method: Symbol::from("+"),
            args: vec![right],
            block: None,
            parenthesized: false,
        },
    )
}

// Compose `"prefix" + a + "mid" + b + ... + "suffix"` from a sequence
// of (string-literal, expr) alternating segments. Returns just the
// concatenation chain.
fn concat_chain(segments: Vec<Expr>) -> Expr {
    let mut iter = segments.into_iter();
    let mut acc = iter.next().expect("concat_chain needs at least one segment");
    for next in iter {
        acc = bin_concat(acc, next);
    }
    acc
}

// Send `id.to_s` for the typed integer-id parameter.
fn id_to_s(id: &Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(id.clone())),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

// Send `instance.<col>` accessor for an instance ivar.
fn instance_field(instance: &Symbol, col_name: &Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(instance.clone())),
            method: col_name.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// `def self._adapter_all`
///
/// ```ruby
/// def self._adapter_all
///   stmt = Db.prepare("SELECT <cols> FROM <table>")
///   results = []
///   while Db.step?(stmt)
///     instance = new
///     instance.<col> = Db.column_<int|text>(stmt, <i>)
///     ...
///     instance.mark_persisted!
///     results << instance
///   end
///   Db.finalize(stmt)
///   results
/// end
/// ```
fn synth_adapter_all(owner: &ClassId, table: &Table) -> MethodDef {
    let stmt = Symbol::from("stmt");
    let results = Symbol::from("results");
    let instance = Symbol::from("instance");
    let db = ClassId(Symbol::from(DB_MOD));
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };

    let sql = format!("SELECT {} FROM {}", select_cols_csv(table), table.name.as_str());
    let stmt_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: stmt.clone() },
            value: db_call(&db, "prepare", vec![lit_str(sql)]),
        },
    );

    let results_init = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: results.clone() },
            value: Expr::new(
                Span::synthetic(),
                ExprNode::Array {
                    elements: vec![],
                    style: crate::expr::ArrayStyle::Brackets,
                },
            ),
        },
    );

    let mut loop_body = hydrate_instance_body(owner, table, &db, &stmt, &instance);
    // results << instance
    loop_body.push(Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(results.clone())),
            method: Symbol::from("<<"),
            args: vec![var_ref(instance)],
            block: None,
            parenthesized: false,
        },
    ));
    let while_loop = Expr::new(
        Span::synthetic(),
        ExprNode::While {
            cond: db_call(&db, "step?", vec![var_ref(stmt.clone())]),
            body: seq(loop_body),
            until_form: false,
        },
    );

    let finalize_call = db_call(&db, "finalize", vec![var_ref(stmt)]);

    let body = seq(vec![stmt_assign, results_init, while_loop, finalize_call, var_ref(results)]);

    MethodDef {
        name: Symbol::from("_adapter_all"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body,
        signature: Some(fn_sig(vec![], Ty::Array { elem: Box::new(owner_ty) })),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

/// `def self._adapter_insert(instance)`
///
/// ```ruby
/// def self._adapter_insert(instance)
///   Db.exec("INSERT INTO <table> (<non_id_cols>) VALUES (" +
///           Db.escape_<text|int>(instance.<col>) + ", " +
///           ... +
///           ")")
///   Db.last_insert_rowid
/// end
/// ```
fn synth_adapter_insert(owner: &ClassId, table: &Table) -> MethodDef {
    let instance = Symbol::from("instance");
    let db = ClassId(Symbol::from(DB_MOD));
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };

    let insertable: Vec<&crate::schema::Column> =
        table.columns.iter().filter(|c| !c.primary_key).collect();
    let cols_csv = insertable
        .iter()
        .map(|c| c.name.as_str().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    // Compose: "INSERT INTO <table> (cols) VALUES (" + escape(c0) + ", " + escape(c1) + ", " + ... + ")"
    let mut segments: Vec<Expr> = vec![lit_str(format!(
        "INSERT INTO {} ({}) VALUES (",
        table.name.as_str(),
        cols_csv
    ))];
    for (idx, col) in insertable.iter().enumerate() {
        if idx > 0 {
            segments.push(lit_str(", ".to_string()));
        }
        let col_ty = ty_of_column(&col.col_type);
        let escape_method = if matches!(col_ty, Ty::Int) {
            "escape_int"
        } else {
            "escape_string"
        };
        segments.push(db_call(
            &db,
            escape_method,
            vec![instance_field(&instance, &col.name)],
        ));
    }
    segments.push(lit_str(")".to_string()));

    let exec_call = db_call(&db, "exec", vec![concat_chain(segments)]);
    let last_id = db_call(&db, "last_insert_rowid", vec![]);

    let body = seq(vec![exec_call, last_id]);

    MethodDef {
        name: Symbol::from("_adapter_insert"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(instance.clone())],
        body,
        signature: Some(fn_sig(vec![(instance, owner_ty)], Ty::Int)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

/// `def self._adapter_update(id, instance)`
fn synth_adapter_update(owner: &ClassId, table: &Table) -> MethodDef {
    let id = Symbol::from("id");
    let instance = Symbol::from("instance");
    let db = ClassId(Symbol::from(DB_MOD));
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };

    let updatable: Vec<&crate::schema::Column> =
        table.columns.iter().filter(|c| !c.primary_key).collect();

    // "UPDATE <table> SET col0 = " + escape(c0) + ", col1 = " + escape(c1) + " WHERE id = " + id.to_s
    let mut segments: Vec<Expr> = vec![lit_str(format!("UPDATE {} SET ", table.name.as_str()))];
    for (idx, col) in updatable.iter().enumerate() {
        let prefix = if idx == 0 {
            format!("{} = ", col.name.as_str())
        } else {
            format!(", {} = ", col.name.as_str())
        };
        segments.push(lit_str(prefix));
        let col_ty = ty_of_column(&col.col_type);
        let escape_method = if matches!(col_ty, Ty::Int) {
            "escape_int"
        } else {
            "escape_string"
        };
        segments.push(db_call(
            &db,
            escape_method,
            vec![instance_field(&instance, &col.name)],
        ));
    }
    segments.push(lit_str(" WHERE id = ".to_string()));
    segments.push(id_to_s(&id));

    let exec_call = db_call(&db, "exec", vec![concat_chain(segments)]);

    MethodDef {
        name: Symbol::from("_adapter_update"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(id.clone()), Param::positional(instance.clone())],
        body: exec_call,
        signature: Some(fn_sig(
            vec![(id, Ty::Int), (instance, owner_ty)],
            Ty::Nil,
        )),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

/// `def self._adapter_delete(id)`
fn synth_adapter_delete(owner: &ClassId, table: &Table) -> MethodDef {
    let id = Symbol::from("id");
    let db = ClassId(Symbol::from(DB_MOD));

    let segments = vec![
        lit_str(format!("DELETE FROM {} WHERE id = ", table.name.as_str())),
        id_to_s(&id),
    ];
    let exec_call = db_call(&db, "exec", vec![concat_chain(segments)]);

    MethodDef {
        name: Symbol::from("_adapter_delete"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(id.clone())],
        body: exec_call,
        signature: Some(fn_sig(vec![(id, Ty::Int)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

/// `def self._adapter_count`
fn synth_adapter_count(owner: &ClassId, table: &Table) -> MethodDef {
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let db = ClassId(Symbol::from(DB_MOD));

    let sql = format!("SELECT COUNT(*) FROM {}", table.name.as_str());
    let stmt_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: stmt.clone() },
            value: db_call(&db, "prepare", vec![lit_str(sql)]),
        },
    );
    let step = db_call(&db, "step?", vec![var_ref(stmt.clone())]);
    let read = db_call(
        &db,
        "column_int",
        vec![var_ref(stmt.clone()), lit_int(0)],
    );
    let result_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: result.clone() },
            value: read,
        },
    );
    let finalize = db_call(&db, "finalize", vec![var_ref(stmt)]);

    let body = seq(vec![stmt_assign, step, result_assign, finalize, var_ref(result)]);

    MethodDef {
        name: Symbol::from("_adapter_count"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body,
        signature: Some(fn_sig(vec![], Ty::Int)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

/// `def self._adapter_exists_by_id?(id)`
fn synth_adapter_exists_by_id(owner: &ClassId, table: &Table) -> MethodDef {
    let id = Symbol::from("id");
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let db = ClassId(Symbol::from(DB_MOD));

    let segments = vec![
        lit_str(format!("SELECT 1 FROM {} WHERE id = ", table.name.as_str())),
        id_to_s(&id),
        lit_str(" LIMIT 1".to_string()),
    ];
    let stmt_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: stmt.clone() },
            value: db_call(&db, "prepare", vec![concat_chain(segments)]),
        },
    );
    let result_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: result.clone() },
            value: db_call(&db, "step?", vec![var_ref(stmt.clone())]),
        },
    );
    let finalize = db_call(&db, "finalize", vec![var_ref(stmt)]);

    let body = seq(vec![stmt_assign, result_assign, finalize, var_ref(result)]);

    MethodDef {
        name: Symbol::from("_adapter_exists_by_id?"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(id.clone())],
        body,
        signature: Some(fn_sig(vec![(id, Ty::Int)], Ty::Bool)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}

/// `def self._adapter_truncate`
fn synth_adapter_truncate(owner: &ClassId, table: &Table) -> MethodDef {
    let db = ClassId(Symbol::from(DB_MOD));
    let sql = format!("DELETE FROM {}", table.name.as_str());
    let exec_call = db_call(&db, "exec", vec![lit_str(sql)]);

    MethodDef {
        name: Symbol::from("_adapter_truncate"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body: exec_call,
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    }
}
