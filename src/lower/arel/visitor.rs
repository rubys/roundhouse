//! Per-backend visitors that turn an `ArelOp` into emitted `Expr`.
//!
//! Phase 1 ships `SqliteVisitor` — the only backend that has a Db
//! primitive shim today (CRuby `sqlite3-ruby` + spinel-FFI
//! `libsqlite3`). Future backends (postgres, libsql variants) will
//! add sibling visitor structs; the IR shape doesn't change.
//!
//! The Expr shapes the visitor emits match what today's
//! `lower::model_to_library::adapter_emit` synthesizes per shape —
//! that's the migration target for Phase 1 step 5 (refactor
//! adapter_emit to build Arel + call this visitor instead of
//! synthesizing inline).

use crate::dialect::AccessorKind;
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode, Literal, LValue};
use crate::ident::{ClassId, Symbol, VarId};
use crate::schema::{Column, ColumnType, Schema, Table};
use crate::span::Span;
use crate::ty::Ty;

use super::ir::{
    ArelOp, Assignment, ColumnSpec, Delete, Insert, Predicate, Select, Update, Value, ValueType,
};

const DB_MOD: &str = "Db";

/// Render an Arel tree into an `Expr` that calls into the per-target
/// `Db` primitive surface. `owner` is the model class that owns the
/// emitted method (used by Select to construct hydrated instances);
/// `schema` carries column ordering for `ColumnSpec::All` expansion
/// and column types for hydration.
pub trait ArelVisitor {
    fn visit(&self, op: &ArelOp, schema: &Schema, owner: &ClassId) -> Expr;
}

/// Visitor for the sqlite backend (CRuby `sqlite3-ruby` + spinel FFI
/// `libsqlite3`). Emits the same Expr shapes today's `adapter_emit`
/// produces.
pub struct SqliteVisitor;

impl ArelVisitor for SqliteVisitor {
    fn visit(&self, op: &ArelOp, schema: &Schema, owner: &ClassId) -> Expr {
        match op {
            ArelOp::Select(s) => visit_select(s, schema, owner),
            ArelOp::Insert(i) => visit_insert(i, schema),
            ArelOp::Update(u) => visit_update(u, schema),
            ArelOp::Delete(d) => visit_delete(d, schema),
        }
    }
}

// ---------------------------------------------------------------------------
// Select — four result shapes, dispatched by ColumnSpec + LimitSpec
// ---------------------------------------------------------------------------

fn visit_select(sel: &Select, schema: &Schema, owner: &ClassId) -> Expr {
    let table = lookup_table(schema, &sel.table.0);
    match &sel.columns {
        ColumnSpec::Count => emit_count(sel, table),
        ColumnSpec::Exists => emit_exists(sel, table),
        ColumnSpec::All => match sel.limit {
            Some(super::ir::LimitSpec(1)) => emit_single_hydrate(sel, table, owner),
            _ => emit_multi_hydrate(sel, table, owner),
        },
        ColumnSpec::Named(_) => {
            // Reserved — no Phase 1 builder produces Named.
            unimplemented!("ColumnSpec::Named is reserved for find_by(<col>); not yet wired")
        }
    }
}

/// `SELECT <cols> FROM <table> [WHERE …] LIMIT 1` →
/// nilable single-instance hydrate.
fn emit_single_hydrate(sel: &Select, table: &Table, owner: &ClassId) -> Expr {
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let instance = Symbol::from("instance");
    let db = ClassId(Symbol::from(DB_MOD));

    let sql = compose_sql_select(sel, table);
    let stmt_assign = assign_var(&stmt, db_call(&db, "prepare", vec![sql]));
    let result_init = assign_var(&result, nil_lit());

    // if Db.step?(stmt) ; instance = new ; <hydrate cols> ; mark_persisted! ; result = instance ; end
    let mut if_body = vec![assign_var(&instance, new_call(owner))];
    push_hydrate_columns(&mut if_body, table, &db, &stmt, &instance);
    if_body.push(send_to(var_ref(&instance), "mark_persisted!", vec![], true));
    if_body.push(assign_var(&result, var_ref(&instance)));

    let if_expr = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: db_call(&db, "step?", vec![var_ref(&stmt)]),
            then_branch: seq(if_body),
            else_branch: nil_lit(),
        },
    );

    let finalize = db_call(&db, "finalize", vec![var_ref(&stmt)]);
    seq(vec![stmt_assign, result_init, if_expr, finalize, var_ref(&result)])
}

/// `SELECT <cols> FROM <table> [WHERE …]` →
/// `Array[<Owner>]` via `while step?` loop.
fn emit_multi_hydrate(sel: &Select, table: &Table, owner: &ClassId) -> Expr {
    let stmt = Symbol::from("stmt");
    let results = Symbol::from("results");
    let instance = Symbol::from("instance");
    let db = ClassId(Symbol::from(DB_MOD));

    let sql = compose_sql_select(sel, table);
    let stmt_assign = assign_var(&stmt, db_call(&db, "prepare", vec![sql]));
    // Empty Array literal carries an explicit `Array<Owner>` type
    // annotation so strict-target emit (Crystal `[] of Owner`)
    // matches the subsequent `results << instance` push semantics.
    // The body-typer would otherwise type `[]` as `Array<Var>` and
    // the Crystal emit's empty-array default (`[] of String`) would
    // mismatch the appended instances.
    let owner_array_ty = crate::ty::Ty::Array {
        elem: Box::new(crate::ty::Ty::Class {
            id: owner.clone(),
            args: vec![],
        }),
    };
    let results_init = assign_var(
        &results,
        crate::lower::typing::with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Array { elements: vec![], style: ArrayStyle::Brackets },
            ),
            owner_array_ty,
        ),
    );

    // while Db.step?(stmt) ; instance = new ; <hydrate> ; mark_persisted! ; results << instance ; end
    let mut loop_body = vec![assign_var(&instance, new_call(owner))];
    push_hydrate_columns(&mut loop_body, table, &db, &stmt, &instance);
    loop_body.push(send_to(var_ref(&instance), "mark_persisted!", vec![], true));
    loop_body.push(send_to(var_ref(&results), "<<", vec![var_ref(&instance)], false));

    let while_loop = Expr::new(
        Span::synthetic(),
        ExprNode::While {
            cond: db_call(&db, "step?", vec![var_ref(&stmt)]),
            body: seq(loop_body),
            until_form: false,
        },
    );

    let finalize = db_call(&db, "finalize", vec![var_ref(&stmt)]);
    seq(vec![stmt_assign, results_init, while_loop, finalize, var_ref(&results)])
}

/// `SELECT COUNT(*) FROM <table> [WHERE …]` → integer scalar.
fn emit_count(sel: &Select, table: &Table) -> Expr {
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let db = ClassId(Symbol::from(DB_MOD));

    let mut segments: Vec<Expr> = vec![lit_str(format!(
        "SELECT COUNT(*) FROM {}",
        table.name.as_str()
    ))];
    push_where_segments(&mut segments, sel.conditions.as_ref(), table);

    let stmt_assign = assign_var(&stmt, db_call(&db, "prepare", vec![concat_chain(segments)]));
    let step = db_call(&db, "step?", vec![var_ref(&stmt)]);
    let read = db_call(&db, "column_int", vec![var_ref(&stmt), lit_int(0)]);
    let result_assign = assign_var(&result, read);
    let finalize = db_call(&db, "finalize", vec![var_ref(&stmt)]);

    seq(vec![stmt_assign, step, result_assign, finalize, var_ref(&result)])
}

/// `SELECT 1 FROM <table> WHERE … LIMIT 1` → bool from `step?`.
fn emit_exists(sel: &Select, table: &Table) -> Expr {
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let db = ClassId(Symbol::from(DB_MOD));

    let mut segments: Vec<Expr> =
        vec![lit_str(format!("SELECT 1 FROM {}", table.name.as_str()))];
    push_where_segments(&mut segments, sel.conditions.as_ref(), table);
    if let Some(super::ir::LimitSpec(n)) = sel.limit {
        segments.push(lit_str(format!(" LIMIT {}", n)));
    }

    let stmt_assign = assign_var(&stmt, db_call(&db, "prepare", vec![concat_chain(segments)]));
    let result_assign = assign_var(&result, db_call(&db, "step?", vec![var_ref(&stmt)]));
    let finalize = db_call(&db, "finalize", vec![var_ref(&stmt)]);

    seq(vec![stmt_assign, result_assign, finalize, var_ref(&result)])
}

// ---------------------------------------------------------------------------
// Insert / Update / Delete — exec-shaped, no hydration
// ---------------------------------------------------------------------------

fn visit_insert(ins: &Insert, schema: &Schema) -> Expr {
    let _ = lookup_table(schema, &ins.table.0); // validates table exists; not consumed beyond that
    let db = ClassId(Symbol::from(DB_MOD));

    let cols_csv = ins
        .assignments
        .iter()
        .map(|a| a.column.as_str().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let mut segments: Vec<Expr> = vec![lit_str(format!(
        "INSERT INTO {} ({}) VALUES (",
        ins.table.0.as_str(),
        cols_csv
    ))];
    for (idx, a) in ins.assignments.iter().enumerate() {
        if idx > 0 {
            segments.push(lit_str(", ".to_string()));
        }
        segments.push(escape_value(&db, &a.value));
    }
    segments.push(lit_str(")".to_string()));

    let exec_call = db_call(&db, "exec", vec![concat_chain(segments)]);
    let last_id = db_call(&db, "last_insert_rowid", vec![]);
    seq(vec![exec_call, last_id])
}

fn visit_update(upd: &Update, schema: &Schema) -> Expr {
    let table = lookup_table(schema, &upd.table.0);
    let db = ClassId(Symbol::from(DB_MOD));

    let mut segments: Vec<Expr> = vec![lit_str(format!("UPDATE {} SET ", upd.table.0.as_str()))];
    for (idx, a) in upd.assignments.iter().enumerate() {
        let prefix = if idx == 0 {
            format!("{} = ", a.column.as_str())
        } else {
            format!(", {} = ", a.column.as_str())
        };
        segments.push(lit_str(prefix));
        segments.push(escape_value(&db, &a.value));
    }
    push_where_segments(&mut segments, upd.conditions.as_ref(), table);
    db_call(&db, "exec", vec![concat_chain(segments)])
}

fn visit_delete(del: &Delete, schema: &Schema) -> Expr {
    let table = lookup_table(schema, &del.table.0);
    let db = ClassId(Symbol::from(DB_MOD));

    let mut segments: Vec<Expr> = vec![lit_str(format!("DELETE FROM {}", del.table.0.as_str()))];
    push_where_segments(&mut segments, del.conditions.as_ref(), table);
    let delete_call = db_call(&db, "exec", vec![concat_chain(segments)]);

    // Truncate (Delete with no WHERE) — also reset the
    // sqlite_sequence row for this table so the next INSERT picks up
    // at id=1 instead of `max-id-ever-seen + 1`. Without this, fixture
    // re-load between tests inserts at id=3, id=4, … and tests that
    // assume id=1 (`Article.find(1)` from `ArticlesFixtures.one`)
    // fail. The schema_sql emitter generates `INTEGER PRIMARY KEY
    // AUTOINCREMENT` for every primary_key column, so sqlite_sequence
    // is guaranteed to exist whenever this table has one — guard on
    // that to avoid hitting "no such table: sqlite_sequence" on
    // schemas with no AUTOINCREMENT tables. sqlite-specific —
    // postgres / other backends would skip this branch in their own
    // visitor.
    let has_primary_key = table.columns.iter().any(|c| c.primary_key);
    if del.conditions.is_none() && has_primary_key {
        let reset = db_call(
            &db,
            "exec",
            vec![lit_str(format!(
                "DELETE FROM sqlite_sequence WHERE name = '{}'",
                del.table.0.as_str()
            ))],
        );
        return seq(vec![delete_call, reset]);
    }
    delete_call
}

// ---------------------------------------------------------------------------
// SQL composition helpers
// ---------------------------------------------------------------------------

/// Build the SELECT prefix `"SELECT <cols> FROM <table>[...]"` plus
/// any WHERE / LIMIT segments, returning the concat chain.
fn compose_sql_select(sel: &Select, table: &Table) -> Expr {
    let cols_csv = match &sel.columns {
        ColumnSpec::All => select_cols_csv(table),
        ColumnSpec::Named(refs) => refs
            .iter()
            .map(|r| r.column.as_str().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        ColumnSpec::Count => "COUNT(*)".to_string(),
        ColumnSpec::Exists => "1".to_string(),
    };
    let mut segments: Vec<Expr> = vec![lit_str(format!(
        "SELECT {} FROM {}",
        cols_csv,
        table.name.as_str()
    ))];
    push_where_segments(&mut segments, sel.conditions.as_ref(), table);
    push_order_segment(&mut segments, &sel.orders);
    if let Some(super::ir::LimitSpec(n)) = sel.limit {
        segments.push(lit_str(format!(" LIMIT {}", n)));
    }
    concat_chain(segments)
}

/// Append `" ORDER BY col1 ASC, col2 DESC"` to the SQL composition
/// when at least one order is present. Empty orders → no segment.
/// Column names emit verbatim (single-table SELECTs only today;
/// joins / aliases land later with the same path).
fn push_order_segment(segments: &mut Vec<Expr>, orders: &[super::ir::Order]) {
    if orders.is_empty() {
        return;
    }
    let parts: Vec<String> = orders
        .iter()
        .map(|o| {
            let dir = match o.direction {
                super::ir::Direction::Asc => "ASC",
                super::ir::Direction::Desc => "DESC",
            };
            format!("{} {}", o.column.column.as_str(), dir)
        })
        .collect();
    segments.push(lit_str(format!(" ORDER BY {}", parts.join(", "))));
}

/// Append `" WHERE <pred-sql>"` segments if conditions are present.
/// Walks the Predicate, emitting literal SQL chunks for column names
/// and operators, and `Db.escape_<ty>(<expr>)` calls for Runtime
/// values. Literal values are baked directly into the SQL string.
fn push_where_segments(segments: &mut Vec<Expr>, preds: Option<&Predicate>, _table: &Table) {
    let Some(pred) = preds else {
        return;
    };
    segments.push(lit_str(" WHERE ".to_string()));
    push_predicate_segments(segments, pred);
}

fn push_predicate_segments(segments: &mut Vec<Expr>, pred: &Predicate) {
    match pred {
        Predicate::Eq(col, val) => {
            segments.push(lit_str(format!("{} = ", col.column.as_str())));
            push_value_segment(segments, val);
        }
        Predicate::And(l, r) => {
            push_predicate_segments(segments, l);
            segments.push(lit_str(" AND ".to_string()));
            push_predicate_segments(segments, r);
        }
        Predicate::Or(l, r) => {
            segments.push(lit_str("(".to_string()));
            push_predicate_segments(segments, l);
            segments.push(lit_str(" OR ".to_string()));
            push_predicate_segments(segments, r);
            segments.push(lit_str(")".to_string()));
        }
    }
}

/// Emit one SQL value segment. Literal values bake into the SQL
/// string directly; Runtime values become `Db.escape_<ty>(<expr>)`.
fn push_value_segment(segments: &mut Vec<Expr>, val: &Value) {
    let db = ClassId(Symbol::from(DB_MOD));
    match val {
        Value::LiteralInt(n) => segments.push(lit_str(n.to_string())),
        Value::LiteralStr(s) => segments.push(lit_str(format!("'{}'", s.replace('\'', "''")))),
        Value::LiteralBool(b) => segments.push(lit_str(if *b { "1".into() } else { "0".into() })),
        Value::LiteralNull => segments.push(lit_str("NULL".into())),
        Value::Runtime { .. } => segments.push(escape_value(&db, val)),
    }
}

/// `Db.escape_<ty>(<expr>)` for a Runtime value; literals stringify
/// inline. Callers that already split literal vs runtime can call
/// `push_value_segment` instead.
fn escape_value(db: &ClassId, val: &Value) -> Expr {
    match val {
        Value::Runtime { expr, ty } => {
            let method = match ty {
                ValueType::Int => "escape_int",
                ValueType::Str => "escape_string",
                ValueType::Bool => "escape_bool",
            };
            db_call(db, method, vec![expr.clone()])
        }
        Value::LiteralInt(n) => lit_str(n.to_string()),
        Value::LiteralStr(s) => lit_str(format!("'{}'", s.replace('\'', "''"))),
        Value::LiteralBool(b) => lit_str(if *b { "1".into() } else { "0".into() }),
        Value::LiteralNull => lit_str("NULL".into()),
    }
}

// ---------------------------------------------------------------------------
// Hydration
// ---------------------------------------------------------------------------

/// Append per-column reads `instance.<col> = Db.column_<int|text>(stmt, <i>)`
/// in schema order. Mirrors `adapter_emit::hydrate_instance_body`.
fn push_hydrate_columns(
    out: &mut Vec<Expr>,
    table: &Table,
    db: &ClassId,
    stmt: &Symbol,
    instance: &Symbol,
) {
    for (i, col) in table.columns.iter().enumerate() {
        let read_method = read_method_for(&col.col_type);
        let read_call = db_call(db, read_method, vec![var_ref(stmt), lit_int(i as i64)]);
        out.push(send_to(
            var_ref(instance),
            &format!("{}=", col.name.as_str()),
            vec![read_call],
            false,
        ));
    }
}

/// Pick `column_int` / `column_bool` / `column_text` based on the
/// schema column's type. Mirrors today's `adapter_emit` branch.
fn read_method_for(t: &ColumnType) -> &'static str {
    match ty_of_column(t) {
        Ty::Int => "column_int",
        Ty::Bool => "column_bool",
        _ => "column_text",
    }
}

fn select_cols_csv(table: &Table) -> String {
    table
        .columns
        .iter()
        .map(|c| c.name.as_str().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Expr builders — local copies of the small helpers also used by
// model_to_library. Phase 1 step 5 collapses adapter_emit; at that
// point the helper set consolidates in one place.
// ---------------------------------------------------------------------------

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

fn send_to(recv: Expr, method: &str, args: Vec<Expr>, parenthesized: bool) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized,
        },
    )
}

fn new_call(owner: &ClassId) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("new"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    )
}

fn assign_var(name: &Symbol, value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: name.clone() },
            value,
        },
    )
}

fn class_const(id: &ClassId) -> Expr {
    let path: Vec<Symbol> = id.0.as_str().split("::").map(Symbol::from).collect();
    Expr::new(Span::synthetic(), ExprNode::Const { path })
}

fn var_ref(name: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: name.clone() })
}

fn lit_str(s: String) -> Expr {
    let mut e = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s } });
    e.ty = Some(Ty::Str);
    e
}

fn lit_int(value: i64) -> Expr {
    let mut e = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value } });
    e.ty = Some(Ty::Int);
    e
}

fn nil_lit() -> Expr {
    let mut e = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
    e.ty = Some(Ty::Nil);
    e
}

fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}

fn concat_chain(segments: Vec<Expr>) -> Expr {
    let mut iter = segments.into_iter();
    let mut acc = iter.next().expect("concat_chain needs at least one segment");
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

/// Schema column type → roundhouse `Ty`. Mirrors
/// `lower::model_to_library::ty_of_column`. Visitor uses the
/// resulting `Ty::Int` to pick `column_int`/`escape_int`.
fn ty_of_column(t: &ColumnType) -> Ty {
    match t {
        ColumnType::Integer | ColumnType::BigInt => Ty::Int,
        ColumnType::Float | ColumnType::Decimal { .. } => Ty::Float,
        ColumnType::String { .. } | ColumnType::Text => Ty::Str,
        ColumnType::Boolean => Ty::Bool,
        ColumnType::Date | ColumnType::DateTime | ColumnType::Time => Ty::Str,
        ColumnType::Binary => Ty::Str,
        ColumnType::Json => Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) },
        ColumnType::Reference { .. } => Ty::Int,
    }
}

// ---------------------------------------------------------------------------

fn lookup_table<'s>(schema: &'s Schema, name: &Symbol) -> &'s Table {
    schema
        .tables
        .get(name)
        .unwrap_or_else(|| panic!("Arel visitor: table {} not in schema", name.as_str()))
}

// Suppress unused-import warnings for symbols that future visitors
// (postgres, libsql) and Phase 2 hooks will consume but that the
// Phase 1 surface doesn't reach.
#[allow(dead_code)]
fn _phase2_anchor(_: AccessorKind, _: EffectSet, _: &Column) {}

#[cfg(test)]
mod tests {
    use super::super::ir::{
        Assignment, ColRef, ColumnSpec, Delete, Insert, LimitSpec, Predicate, Select, Update, Value,
        ValueType,
    };
    use super::*;
    use crate::ident::TableRef;
    use crate::schema::{Column, Schema, Table};
    use indexmap::IndexMap;

    // Two-column "articles" table: id (Int), title (Str).
    fn fixture_schema() -> (Schema, ClassId) {
        let mut tables = IndexMap::new();
        tables.insert(
            Symbol::from("articles"),
            Table {
                name: Symbol::from("articles"),
                columns: vec![
                    Column {
                        name: Symbol::from("id"),
                        col_type: ColumnType::Integer,
                        nullable: false,
                        default: None,
                        primary_key: true,
                    },
                    Column {
                        name: Symbol::from("title"),
                        col_type: ColumnType::Text,
                        nullable: false,
                        default: None,
                        primary_key: false,
                    },
                ],
                indexes: vec![],
                foreign_keys: vec![],
            },
        );
        (Schema { tables }, ClassId(Symbol::from("Article")))
    }

    fn id_col() -> ColRef {
        ColRef { table: TableRef(Symbol::from("articles")), column: Symbol::from("id") }
    }

    // Rough shape probe — confirms the visitor returns a Seq whose
    // body matches today's `_adapter_*` skeleton (prepare → init →
    // body → finalize → return) for hydrate ops, and a Send/Seq for
    // exec ops. We assert on the outer node kind and stmt count;
    // byte-exact equivalence comes via the Ruby emit path in step 6.
    fn outer_kind(e: &Expr) -> &'static str {
        match e.node.as_ref() {
            ExprNode::Seq { .. } => "seq",
            ExprNode::Send { .. } => "send",
            _ => "other",
        }
    }

    fn seq_len(e: &Expr) -> usize {
        match e.node.as_ref() {
            ExprNode::Seq { exprs } => exprs.len(),
            _ => 0,
        }
    }

    #[test]
    fn select_all_no_limit_emits_array_hydrate() {
        let (schema, owner) = fixture_schema();
        let op = ArelOp::Select(Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::All,
            conditions: None,
            orders: vec![],
            limit: None,
            joins: vec![],
        });
        let body = SqliteVisitor.visit(&op, &schema, &owner);
        // stmt = prepare ; results = [] ; while step? { ... } ; finalize ; results
        assert_eq!(outer_kind(&body), "seq");
        assert_eq!(seq_len(&body), 5);
    }

    #[test]
    fn select_all_limit_one_emits_single_hydrate() {
        let (schema, owner) = fixture_schema();
        let op = ArelOp::Select(Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::All,
            conditions: Some(Predicate::Eq(
                id_col(),
                Value::Runtime { expr: var_ref(&Symbol::from("id")), ty: ValueType::Int },
            )),
            orders: vec![],
            limit: Some(LimitSpec(1)),
            joins: vec![],
        });
        let body = SqliteVisitor.visit(&op, &schema, &owner);
        // stmt = prepare ; result = nil ; if step? { ... } ; finalize ; result
        assert_eq!(outer_kind(&body), "seq");
        assert_eq!(seq_len(&body), 5);
    }

    #[test]
    fn select_count_emits_int_scalar() {
        let (schema, owner) = fixture_schema();
        let op = ArelOp::Select(Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::Count,
            conditions: None,
            orders: vec![],
            limit: None,
            joins: vec![],
        });
        let body = SqliteVisitor.visit(&op, &schema, &owner);
        // stmt = prepare ; step? ; result = column_int ; finalize ; result
        assert_eq!(outer_kind(&body), "seq");
        assert_eq!(seq_len(&body), 5);
    }

    #[test]
    fn select_exists_emits_bool_from_step() {
        let (schema, owner) = fixture_schema();
        let op = ArelOp::Select(Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::Exists,
            conditions: Some(Predicate::Eq(
                id_col(),
                Value::Runtime { expr: var_ref(&Symbol::from("id")), ty: ValueType::Int },
            )),
            orders: vec![],
            limit: Some(LimitSpec(1)),
            joins: vec![],
        });
        let body = SqliteVisitor.visit(&op, &schema, &owner);
        // stmt = prepare ; result = step? ; finalize ; result
        assert_eq!(outer_kind(&body), "seq");
        assert_eq!(seq_len(&body), 4);
    }

    #[test]
    fn insert_emits_exec_then_last_insert_rowid() {
        let (schema, _) = fixture_schema();
        let op = ArelOp::Insert(Insert {
            table: TableRef(Symbol::from("articles")),
            assignments: vec![Assignment {
                column: Symbol::from("title"),
                value: Value::Runtime {
                    expr: var_ref(&Symbol::from("title")),
                    ty: ValueType::Str,
                },
            }],
        });
        let body = SqliteVisitor.visit(&op, &schema, &ClassId(Symbol::from("Article")));
        // exec ; last_insert_rowid
        assert_eq!(outer_kind(&body), "seq");
        assert_eq!(seq_len(&body), 2);
    }

    #[test]
    fn update_emits_single_exec() {
        let (schema, _) = fixture_schema();
        let op = ArelOp::Update(Update {
            table: TableRef(Symbol::from("articles")),
            assignments: vec![Assignment {
                column: Symbol::from("title"),
                value: Value::Runtime {
                    expr: var_ref(&Symbol::from("title")),
                    ty: ValueType::Str,
                },
            }],
            conditions: Some(Predicate::Eq(
                id_col(),
                Value::Runtime { expr: var_ref(&Symbol::from("id")), ty: ValueType::Int },
            )),
        });
        let body = SqliteVisitor.visit(&op, &schema, &ClassId(Symbol::from("Article")));
        // single Db.exec(...) Send
        assert_eq!(outer_kind(&body), "send");
    }

    #[test]
    fn delete_emits_single_exec() {
        let (schema, _) = fixture_schema();
        let op = ArelOp::Delete(Delete {
            table: TableRef(Symbol::from("articles")),
            conditions: Some(Predicate::Eq(
                id_col(),
                Value::Runtime { expr: var_ref(&Symbol::from("id")), ty: ValueType::Int },
            )),
        });
        let body = SqliteVisitor.visit(&op, &schema, &ClassId(Symbol::from("Article")));
        assert_eq!(outer_kind(&body), "send");
    }

    #[test]
    fn delete_no_conditions_is_truncate() {
        // Truncate over an AUTOINCREMENT-bearing table emits a Seq:
        // `Db.exec("DELETE FROM articles")` then
        // `Db.exec("DELETE FROM sqlite_sequence WHERE name = 'articles'")`
        // so the next INSERT restarts at id=1. Fixture has a
        // primary_key column → both stmts emit.
        let (schema, _) = fixture_schema();
        let op = ArelOp::Delete(Delete {
            table: TableRef(Symbol::from("articles")),
            conditions: None,
        });
        let body = SqliteVisitor.visit(&op, &schema, &ClassId(Symbol::from("Article")));
        assert_eq!(outer_kind(&body), "seq");
        assert_eq!(seq_len(&body), 2);
    }

    #[test]
    fn select_with_order_emits_order_by_segment() {
        // Article.all.order(created_at: :desc) — multi-hydrate Seq
        // shape (no LIMIT), but the prepare's SQL string must carry
        // the ORDER BY clause.
        use crate::ident::TableRef;
        let (schema, owner) = fixture_schema();
        let op = ArelOp::Select(Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::All,
            conditions: None,
            orders: vec![super::super::ir::Order {
                column: super::super::ir::ColRef {
                    table: TableRef(Symbol::from("articles")),
                    column: Symbol::from("title"),
                },
                direction: super::super::ir::Direction::Desc,
            }],
            limit: None,
            joins: vec![],
        });
        let body = SqliteVisitor.visit(&op, &schema, &owner);
        // Walk the body to the prepare call's SQL literal and assert
        // it carries " ORDER BY title DESC". The body is a Seq whose
        // first stmt is `stmt = Db.prepare(<sql>)`.
        let ExprNode::Seq { exprs } = body.node.as_ref() else {
            panic!("expected Seq body");
        };
        let ExprNode::Assign { value: prepare_call, .. } = exprs[0].node.as_ref() else {
            panic!("expected Assign");
        };
        let ExprNode::Send { args, .. } = prepare_call.node.as_ref() else {
            panic!("expected Send to Db.prepare");
        };
        // The SQL is a concat chain; flatten and stringify literal segments.
        fn flatten_lits(e: &Expr, out: &mut String) {
            match e.node.as_ref() {
                ExprNode::Lit { value: Literal::Str { value } } => out.push_str(value),
                ExprNode::Send { recv: Some(left), args, .. } => {
                    flatten_lits(left, out);
                    for a in args { flatten_lits(a, out); }
                }
                _ => out.push_str("<expr>"),
            }
        }
        let mut sql = String::new();
        flatten_lits(&args[0], &mut sql);
        assert!(sql.contains("ORDER BY title DESC"), "expected ORDER BY in:\n{sql}");
    }

    #[test]
    fn select_with_two_orders_emits_csv_order_by() {
        use crate::ident::TableRef;
        let (schema, owner) = fixture_schema();
        let op = ArelOp::Select(Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::All,
            conditions: None,
            orders: vec![
                super::super::ir::Order {
                    column: super::super::ir::ColRef {
                        table: TableRef(Symbol::from("articles")),
                        column: Symbol::from("title"),
                    },
                    direction: super::super::ir::Direction::Asc,
                },
                super::super::ir::Order {
                    column: super::super::ir::ColRef {
                        table: TableRef(Symbol::from("articles")),
                        column: Symbol::from("id"),
                    },
                    direction: super::super::ir::Direction::Desc,
                },
            ],
            limit: None,
            joins: vec![],
        });
        let body = SqliteVisitor.visit(&op, &schema, &owner);
        let ExprNode::Seq { exprs } = body.node.as_ref() else { panic!() };
        let ExprNode::Assign { value: prepare_call, .. } = exprs[0].node.as_ref() else { panic!() };
        let ExprNode::Send { args, .. } = prepare_call.node.as_ref() else { panic!() };
        fn flatten_lits(e: &Expr, out: &mut String) {
            match e.node.as_ref() {
                ExprNode::Lit { value: Literal::Str { value } } => out.push_str(value),
                ExprNode::Send { recv: Some(left), args, .. } => {
                    flatten_lits(left, out);
                    for a in args { flatten_lits(a, out); }
                }
                _ => out.push_str("<expr>"),
            }
        }
        let mut sql = String::new();
        flatten_lits(&args[0], &mut sql);
        assert!(sql.contains("ORDER BY title ASC, id DESC"), "expected CSV ORDER BY in:\n{sql}");
    }

    #[test]
    fn select_where_predicate_with_runtime_value_round_trips() {
        let (schema, owner) = fixture_schema();
        let op = ArelOp::Select(Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::All,
            conditions: Some(Predicate::Eq(
                id_col(),
                Value::Runtime {
                    expr: Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from("id") }),
                    ty: ValueType::Int,
                },
            )),
            orders: vec![],
            limit: None,
            joins: vec![],
        });
        // No-limit + ColumnSpec::All + WHERE → multi-hydrate (the
        // has_many proxy shape: `Comment.where(article_id: @id)`).
        let body = SqliteVisitor.visit(&op, &schema, &owner);
        assert_eq!(outer_kind(&body), "seq");
        assert_eq!(seq_len(&body), 5);
    }
}
