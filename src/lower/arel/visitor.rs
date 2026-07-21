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
use crate::expr::{ArrayStyle, BlockStyle, Expr, ExprNode, Literal, LValue};
use crate::ident::{ClassId, Symbol, VarId};
use crate::schema::{Column, Schema, Table};
use crate::span::Span;
use crate::ty::Ty;

use super::ir::{
    ArelOp, ColumnSpec, Delete, Insert, Predicate, PreloadDirective, Select, Update,
    Value, ValueType,
};

const DB_MOD: &str = "Db";

/// Prototype gate for placeholder-bind emit (roundhouse#12, the
/// "planned follow-on" the Db shims name). When
/// `ROUNDHOUSE_PARAM_BINDS=1`, the `Db.prepare` read paths
/// (single/multi hydrate, count, exists) render *runtime* WHERE values
/// as `?` placeholders plus `Db.bind_*` calls after prepare — so the
/// prepared-statement cache keys on the static query shape
/// (`WHERE id = ?`) instead of per-value (`WHERE id = 3`). Compile-time
/// literals stay inline (they're already part of the static shape).
///
/// Default OFF ⇒ byte-identical inline-escape emit for every target.
/// Only the spinel + cruby `Db` shims implement `bind_*` today; the
/// other targets gain it on real rollout, at which point this gate is
/// replaced by a per-target capability flag threaded from the driver.
/// Env-var gating is deliberate prototype scaffolding: it keeps the
/// change behind one switch so it can be measured on the spinel lane
/// without touching the other ten targets' shims.
///
/// Write paths (INSERT/UPDATE/DELETE) are intentionally NOT
/// parameterized: they go through `Db.exec` (`sqlite3_exec`), which
/// never populates the prepared-statement cache — so binding them buys
/// nothing here.
pub(crate) fn param_binds_enabled() -> bool {
    std::env::var("ROUNDHOUSE_PARAM_BINDS")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// One deferred bind: the unrendered value expr plus the `ValueType`
/// that picks `bind_int` / `bind_text` / `bind_bool`. Accumulated by the
/// SQL composers alongside the `?` they emit, then drained into
/// `Db.bind_*(stmt, i, expr)` calls right after the `Db.prepare`.
struct Bind {
    expr: Expr,
    ty: ValueType,
}

/// `Db.bind_<ty>(stmt, <1-based idx>, <expr>)` for each accumulated
/// bind, in placeholder order (sqlite bind indices are 1-based). The
/// per-value type is known at composition time, so each bind is
/// monomorphic — no heterogeneous bind bag on the hot path.
fn emit_bind_calls(stmt: &Symbol, binds: &[Bind]) -> Vec<Expr> {
    let db = ClassId(Symbol::from(DB_MOD));
    binds
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let method = match b.ty {
                ValueType::Int => "bind_int",
                ValueType::Str => "bind_text",
                ValueType::Bool => "bind_bool",
            };
            db_call(
                &db,
                method,
                vec![var_ref(stmt), lit_int((i + 1) as i64), b.expr.clone()],
            )
        })
        .collect()
}

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
    // Read the placeholder-bind gate once per Select. All four prepare
    // paths honor it; a false value reproduces the inline-escape emit
    // byte-for-byte.
    let param = param_binds_enabled();
    match &sel.columns {
        ColumnSpec::Count => emit_count(sel, table, param),
        ColumnSpec::Exists => emit_exists(sel, table, param),
        ColumnSpec::All => match sel.limit {
            Some(super::ir::LimitSpec(1)) => emit_single_hydrate(sel, table, owner, param),
            _ => emit_multi_hydrate(sel, table, owner, schema, param),
        },
        ColumnSpec::Named(_) => {
            // Reserved — no Phase 1 builder produces Named yet (it's for
            // find_by(<col>)). Degrade instead of crashing: report the
            // gap to the emit sink (so the transpile path surfaces and
            // gates on it) and return a stub node annotated with the
            // diagnostic, so every per-target emitter drops a raise stub
            // at the site via the `Expr.diagnostic` short-circuit.
            let detail = "find_by(<col>) projection not yet wired";
            // The arel IR doesn't carry spans — synthetic until it does.
            crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                crate::span::Span::synthetic(),
                None,
                "ColumnSpec::Named",
                detail,
            ));
            let mut stub = nil_lit();
            stub.diagnostic = Some(crate::diagnostic::DiagnosticKind::Unsupported {
                target: None,
                construct: Symbol::from("ColumnSpec::Named"),
                detail: detail.to_string(),
            });
            stub
        }
    }
}

/// `SELECT <cols> FROM <table> [WHERE …] LIMIT 1` →
/// nilable single-instance hydrate.
fn emit_single_hydrate(sel: &Select, table: &Table, owner: &ClassId, param: bool) -> Expr {
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let db = ClassId(Symbol::from(DB_MOD));

    let mut binds = Vec::new();
    let sql = compose_sql_select(sel, table, param, &mut binds);
    let stmt_assign = assign_var(&stmt, db_call(&db, "prepare", vec![sql]));
    let result_init = assign_var(&result, nil_lit());

    // if Db.step?(stmt) ; result = <Owner>.from_stmt(stmt) ; end
    // (`from_stmt` news the instance, reads every column, and marks it
    // persisted — see `synth_from_stmt`. Sound here because a single
    // hydrate is always `ColumnSpec::All`.)
    let if_body = vec![assign_var(&result, model_from_stmt(owner, &stmt))];

    let if_expr = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: db_call(&db, "step?", vec![var_ref(&stmt)]),
            then_branch: seq(if_body),
            else_branch: nil_lit(),
        },
    );

    let finalize = db_call(&db, "finalize", vec![var_ref(&stmt)]);
    // stmt = prepare ; [bind …] ; result = nil ; if step? {…} ; finalize ; result
    let mut stmts = vec![stmt_assign];
    stmts.extend(emit_bind_calls(&stmt, &binds));
    stmts.push(result_init);
    stmts.push(if_expr);
    stmts.push(finalize);
    stmts.push(var_ref(&result));
    seq(stmts)
}

/// `SELECT <cols> FROM <table> [WHERE …]` →
/// `Array[<Owner>]` via `while step?` loop.
fn emit_multi_hydrate(
    sel: &Select,
    table: &Table,
    owner: &ClassId,
    schema: &Schema,
    param: bool,
) -> Expr {
    let stmt = Symbol::from("stmt");
    let results = Symbol::from("results");
    let db = ClassId(Symbol::from(DB_MOD));

    let mut binds = Vec::new();
    let sql = compose_sql_select(sel, table, param, &mut binds);
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

    // while Db.step?(stmt) ; results << <Owner>.from_stmt(stmt) ; end
    // (`from_stmt` news + hydrates + marks persisted — see
    // `synth_from_stmt`. Sound because a multi hydrate is `ColumnSpec::All`.)
    let loop_body = vec![send_to(
        var_ref(&results),
        "<<",
        vec![model_from_stmt(owner, &stmt)],
        false,
    )];

    let while_loop = Expr::new(
        Span::synthetic(),
        ExprNode::While {
            cond: db_call(&db, "step?", vec![var_ref(&stmt)]),
            body: seq(loop_body),
            until_form: false,
        },
    );

    let finalize = db_call(&db, "finalize", vec![var_ref(&stmt)]);

    // Base hydrate, then any `includes(:assoc)` preloads (issue #27).
    // Each preload appends a batched `WHERE fk IN (ids)` query +
    // distribute loop that fills the parents' association caches, all
    // operating on `results` before it's returned. With no preloads
    // this is byte-identical to the pre-#27 5-stmt Seq.
    // Binds for the MAIN query land right after its prepare, before the
    // hydrate loop. Preload sub-queries build their own statements and
    // keep the inline `IN (…)` list (variable arity — not parameterized
    // here; see the note in `push_preload_stmts`).
    let mut stmts = vec![stmt_assign];
    stmts.extend(emit_bind_calls(&stmt, &binds));
    stmts.push(results_init);
    stmts.push(while_loop);
    stmts.push(finalize);
    for directive in &sel.preloads {
        push_preload_stmts(&mut stmts, directive, schema, owner, &results);
    }
    stmts.push(var_ref(&results));
    seq(stmts)
}

/// Emit the eager-load steps for one `includes(:assoc)` directive,
/// appending to `out`. Given the parent rows already hydrated into
/// `parent_results`, this:
///   1. collects parent ids,
///   2. runs ONE `SELECT … WHERE fk IN (ids)` over the target table,
///   3. distributes the loaded rows into each parent via the
///      `_preload_<assoc>` setter the model lowerer synthesized.
/// Turns Rails' eager-load into 2 queries total instead of 1 + N.
fn push_preload_stmts(
    out: &mut Vec<Expr>,
    directive: &PreloadDirective,
    schema: &Schema,
    parent_owner: &ClassId,
    parent_results: &Symbol,
) {
    let _ = parent_owner;
    let db = ClassId(Symbol::from(DB_MOD));
    let target_table = lookup_table(schema, &directive.target_table.0);
    let assoc = directive.name.as_str();

    let ids = Symbol::from(format!("__{}_ids", assoc));
    let pstmt = Symbol::from(format!("__{}_stmt", assoc));
    let loaded = Symbol::from(format!("__{}_loaded", assoc));

    // ids = parent_results.map { |a| a.id }
    let map_block = block1(
        "a",
        send_to(var_ref(&Symbol::from("a")), "id", vec![], false),
    );
    out.push(assign_var(&ids, send_block(var_ref(parent_results), "map", map_block)));

    // pstmt = Db.prepare("SELECT <cols> FROM <tbl> WHERE <fk> IN (" + Db.escape_int_list(ids) + ")")
    let sql = concat_chain(vec![
        lit_str(format!(
            "SELECT {} FROM {} WHERE {} IN (",
            select_cols_csv(target_table),
            target_table.name.as_str(),
            directive.foreign_key.as_str(),
        )),
        db_call(&db, "escape_int_list", vec![var_ref(&ids)]),
        lit_str(")".to_string()),
    ]);
    out.push(assign_var(&pstmt, db_call(&db, "prepare", vec![sql])));

    // loaded = [] (typed Array<Target> so the `<<` push types cleanly)
    let elem_ty = Ty::Class { id: directive.target_class.clone(), args: vec![] };
    let loaded_init = crate::lower::typing::with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array { elements: vec![], style: ArrayStyle::Brackets },
        ),
        Ty::Array { elem: Box::new(elem_ty) },
    );
    out.push(assign_var(&loaded, loaded_init));

    // while Db.step?(pstmt) { loaded << Target.from_stmt(pstmt) }
    // The preload SQL is built from `select_cols_csv(target_table)` —
    // full row in declaration order — so it's `ColumnSpec::All`-shaped
    // by construction and `from_stmt`'s positional reads line up.
    let loop_body = vec![send_to(
        var_ref(&loaded),
        "<<",
        vec![model_from_stmt(&directive.target_class, &pstmt)],
        false,
    )];
    out.push(Expr::new(
        Span::synthetic(),
        ExprNode::While {
            cond: db_call(&db, "step?", vec![var_ref(&pstmt)]),
            body: seq(loop_body),
            until_form: false,
        },
    ));

    // Db.finalize(pstmt)
    out.push(db_call(&db, "finalize", vec![var_ref(&pstmt)]));

    // Distribute, grouping by FK with a portable nested loop rather than
    // `loaded.select { … }` — Ruby's `Array#select` has no universal
    // emitter mapping (Go has no `.Select`):
    //   parent_results.each do |a|
    //     group = []                       # Array<Target>
    //     loaded.each { |r| group << r if r.<fk> == a.id }
    //     a._preload_<assoc>(group)
    //   end
    let group = Symbol::from(format!("__{}_group", assoc));
    let match_pred = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(send_to(
                var_ref(&Symbol::from("r")),
                directive.foreign_key.as_str(),
                vec![],
                false,
            )),
            method: Symbol::from("=="),
            args: vec![send_to(var_ref(&Symbol::from("a")), "id", vec![], false)],
            block: None,
            parenthesized: false,
        },
    );
    let push_if = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: match_pred,
            then_branch: send_to(var_ref(&group), "<<", vec![var_ref(&Symbol::from("r"))], false),
            else_branch: nil_lit(),
        },
    );
    let group_init = crate::lower::typing::with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array { elements: vec![], style: ArrayStyle::Brackets },
        ),
        Ty::Array { elem: Box::new(Ty::Class { id: directive.target_class.clone(), args: vec![] }) },
    );
    let setter = format!("_preload_{}", assoc);
    let each_block = block1(
        "a",
        seq(vec![
            assign_var(&group, group_init),
            send_block(var_ref(&loaded), "each", block1("r", push_if)),
            send_to(var_ref(&Symbol::from("a")), &setter, vec![var_ref(&group)], true),
        ]),
    );
    out.push(send_block(var_ref(parent_results), "each", each_block));
}

/// `SELECT COUNT(*) FROM <table> [WHERE …]` → integer scalar.
fn emit_count(sel: &Select, table: &Table, param: bool) -> Expr {
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let db = ClassId(Symbol::from(DB_MOD));

    let mut segments: Vec<Expr> = vec![lit_str(format!(
        "SELECT COUNT(*) FROM {}",
        table.name.as_str()
    ))];
    let mut binds = Vec::new();
    push_where_segments(&mut segments, sel.conditions.as_ref(), table, param, &mut binds);

    let stmt_assign = assign_var(&stmt, db_call(&db, "prepare", vec![concat_chain(segments)]));
    let step = db_call(&db, "step?", vec![var_ref(&stmt)]);
    let read = db_call(&db, "column_int", vec![var_ref(&stmt), lit_int(0)]);
    let result_assign = assign_var(&result, read);
    let finalize = db_call(&db, "finalize", vec![var_ref(&stmt)]);

    // stmt = prepare ; [bind …] ; step? ; result = column_int ; finalize ; result
    let mut stmts = vec![stmt_assign];
    stmts.extend(emit_bind_calls(&stmt, &binds));
    stmts.push(step);
    stmts.push(result_assign);
    stmts.push(finalize);
    stmts.push(var_ref(&result));
    seq(stmts)
}

/// `SELECT 1 FROM <table> WHERE … LIMIT 1` → bool from `step?`.
fn emit_exists(sel: &Select, table: &Table, param: bool) -> Expr {
    let stmt = Symbol::from("stmt");
    let result = Symbol::from("result");
    let db = ClassId(Symbol::from(DB_MOD));

    let mut segments: Vec<Expr> =
        vec![lit_str(format!("SELECT 1 FROM {}", table.name.as_str()))];
    let mut binds = Vec::new();
    push_where_segments(&mut segments, sel.conditions.as_ref(), table, param, &mut binds);
    if let Some(super::ir::LimitSpec(n)) = sel.limit {
        segments.push(lit_str(format!(" LIMIT {}", n)));
    }

    let stmt_assign = assign_var(&stmt, db_call(&db, "prepare", vec![concat_chain(segments)]));
    let result_assign = assign_var(&result, db_call(&db, "step?", vec![var_ref(&stmt)]));
    let finalize = db_call(&db, "finalize", vec![var_ref(&stmt)]);

    // stmt = prepare ; [bind …] ; result = step? ; finalize ; result
    let mut stmts = vec![stmt_assign];
    stmts.extend(emit_bind_calls(&stmt, &binds));
    stmts.push(result_assign);
    stmts.push(finalize);
    stmts.push(var_ref(&result));
    seq(stmts)
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
    // Writes go through `Db.exec` (`sqlite3_exec`), which never touches
    // the prepared-statement cache — so the WHERE stays inline-escaped
    // regardless of the placeholder-bind gate.
    push_where_segments(&mut segments, upd.conditions.as_ref(), table, false, &mut Vec::new());
    db_call(&db, "exec", vec![concat_chain(segments)])
}

fn visit_delete(del: &Delete, schema: &Schema) -> Expr {
    let table = lookup_table(schema, &del.table.0);
    let db = ClassId(Symbol::from(DB_MOD));

    let mut segments: Vec<Expr> = vec![lit_str(format!("DELETE FROM {}", del.table.0.as_str()))];
    // Exec path — inline WHERE (see visit_update note); not cached.
    push_where_segments(&mut segments, del.conditions.as_ref(), table, false, &mut Vec::new());
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
fn compose_sql_select(sel: &Select, table: &Table, param: bool, binds: &mut Vec<Bind>) -> Expr {
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
    push_where_segments(&mut segments, sel.conditions.as_ref(), table, param, binds);
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
fn push_where_segments(
    segments: &mut Vec<Expr>,
    preds: Option<&Predicate>,
    _table: &Table,
    param: bool,
    binds: &mut Vec<Bind>,
) {
    let Some(pred) = preds else {
        return;
    };
    segments.push(lit_str(" WHERE ".to_string()));
    push_predicate_segments(segments, pred, param, binds);
}

fn push_predicate_segments(
    segments: &mut Vec<Expr>,
    pred: &Predicate,
    param: bool,
    binds: &mut Vec<Bind>,
) {
    match pred {
        // SQL equality never matches NULL (`col = NULL` is three-valued
        // unknown, i.e. no rows); a literal-nil condition is the IS NULL
        // form, as Rails renders it.
        Predicate::Eq(col, Value::LiteralNull) => {
            segments.push(lit_str(format!("{} IS NULL", col.column.as_str())));
        }
        Predicate::Eq(col, val) => {
            segments.push(lit_str(format!("{} = ", col.column.as_str())));
            push_value_segment(segments, val, param, binds);
        }
        Predicate::And(l, r) => {
            push_predicate_segments(segments, l, param, binds);
            segments.push(lit_str(" AND ".to_string()));
            push_predicate_segments(segments, r, param, binds);
        }
        Predicate::Or(l, r) => {
            segments.push(lit_str("(".to_string()));
            push_predicate_segments(segments, l, param, binds);
            segments.push(lit_str(" OR ".to_string()));
            push_predicate_segments(segments, r, param, binds);
            segments.push(lit_str(")".to_string()));
        }
    }
}

/// Emit one SQL value segment. Literal values bake into the SQL string
/// directly (they're part of the static query shape either way). A
/// Runtime value becomes `Db.escape_<ty>(<expr>)` inline when `param`
/// is off, or a `?` placeholder + a recorded `Bind` when on — so the
/// composed SQL keys the prepared-statement cache by shape, not value.
fn push_value_segment(segments: &mut Vec<Expr>, val: &Value, param: bool, binds: &mut Vec<Bind>) {
    let db = ClassId(Symbol::from(DB_MOD));
    match val {
        Value::LiteralInt(n) => segments.push(lit_str(n.to_string())),
        Value::LiteralStr(s) => segments.push(lit_str(format!("'{}'", s.replace('\'', "''")))),
        Value::LiteralBool(b) => segments.push(lit_str(if *b { "1".into() } else { "0".into() })),
        Value::LiteralNull => segments.push(lit_str("NULL".into())),
        Value::Runtime { expr, ty } if param => {
            segments.push(lit_str("?".to_string()));
            binds.push(Bind { expr: expr.clone(), ty: *ty });
        }
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

/// `<Owner>.from_stmt(stmt)` — the synthesized positional factory
/// (`model_to_library::schema::synth_from_stmt`) that news an instance,
/// reads every schema column from the prepared statement, marks it
/// persisted, and returns it. Replaces the inline
/// `new + per-column read + mark_persisted!` block at every
/// `ColumnSpec::All` hydrate site (single, multi, and eager-load
/// preload). The per-column read logic now lives once, in `from_stmt`'s
/// body, rather than being re-emitted at each query site.
fn model_from_stmt(owner: &ClassId, stmt: &Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(owner)),
            method: Symbol::from("from_stmt"),
            args: vec![var_ref(stmt)],
            block: None,
            parenthesized: true,
        },
    )
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

/// A single-param brace block `{ |name| <body> }`.
fn block1(param: &str, body: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![Symbol::from(param)],
            block_param: None,
            body,
            block_style: BlockStyle::Brace,
        },
    )
}

/// `recv.method { <block> }` — a Send carrying a block, no args.
fn send_block(recv: Expr, method: &str, block: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![],
            block: Some(block),
            parenthesized: false,
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

/// Fold SQL segments into a `+` chain, first merging adjacent string
/// literals: the builders above push prefix / column / separator
/// pieces separately, which would otherwise emit pure-literal runs
/// like `"UPDATE t SET " + "a = " + …` in every target (flagged in
/// the #67 review). `pub(crate)` — `model_to_library::adapter_emit`
/// folds its SQL through the same helper.
pub(crate) fn concat_chain(segments: Vec<Expr>) -> Expr {
    let mut merged: Vec<Expr> = Vec::with_capacity(segments.len());
    for seg in segments {
        let text = match seg.node.as_ref() {
            ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
            _ => None,
        };
        if let Some(text) = text {
            if let Some(last) = merged.last_mut() {
                if let ExprNode::Lit { value: Literal::Str { value } } = last.node.as_mut() {
                    value.push_str(&text);
                    continue;
                }
            }
        }
        merged.push(seg);
    }
    let mut iter = merged.into_iter();
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
    use crate::schema::{Column, ColumnType, Schema, Table};
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
            preloads: vec![],
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
            preloads: vec![],
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
            preloads: vec![],
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
            preloads: vec![],
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
            preloads: vec![],
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
            preloads: vec![],
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
            preloads: vec![],
        });
        // No-limit + ColumnSpec::All + WHERE → multi-hydrate (the
        // has_many proxy shape: `Comment.where(article_id: @id)`).
        let body = SqliteVisitor.visit(&op, &schema, &owner);
        assert_eq!(outer_kind(&body), "seq");
        assert_eq!(seq_len(&body), 5);
    }

    // ---- placeholder-bind emit (ROUNDHOUSE_PARAM_BINDS) -------------
    //
    // These drive the emitters directly with `param = true` rather than
    // flipping the process-global env var (which would race the other
    // tests in this binary). `param_binds_enabled()` selects the same
    // code path at runtime.

    /// Flatten the literal-string segments of a `+` concat chain into
    /// `out`; non-literal segments (escape calls, bind exprs) render as
    /// the sentinel `«expr»` so their absence/presence is assertable.
    fn flatten_sql(e: &Expr, out: &mut String) {
        match e.node.as_ref() {
            ExprNode::Lit { value: Literal::Str { value } } => out.push_str(value),
            ExprNode::Send { recv: Some(left), args, method, .. } if method.as_str() == "+" => {
                flatten_sql(left, out);
                for a in args {
                    flatten_sql(a, out);
                }
            }
            _ => out.push_str("«expr»"),
        }
    }

    /// The SQL string handed to the first `Db.prepare(...)` in a Seq
    /// body (stmt = Db.prepare(<sql>) is always exprs[0]).
    fn prepare_sql(body: &Expr) -> String {
        let ExprNode::Seq { exprs } = body.node.as_ref() else { panic!("expected Seq") };
        let ExprNode::Assign { value, .. } = exprs[0].node.as_ref() else {
            panic!("expected `stmt = …`")
        };
        let ExprNode::Send { args, .. } = value.node.as_ref() else {
            panic!("expected Db.prepare(…)")
        };
        let mut s = String::new();
        flatten_sql(&args[0], &mut s);
        s
    }

    /// Bare `Db.<method>(...)` statement calls in a Seq body, in order
    /// (skips the `stmt = Db.prepare` Assign and any control flow).
    fn db_stmt_calls(body: &Expr) -> Vec<(String, Vec<Expr>)> {
        let ExprNode::Seq { exprs } = body.node.as_ref() else { return vec![] };
        exprs
            .iter()
            .filter_map(|e| {
                if let ExprNode::Send { recv: Some(r), method, args, .. } = e.node.as_ref() {
                    if matches!(r.node.as_ref(), ExprNode::Const { .. }) {
                        return Some((method.as_str().to_string(), args.clone()));
                    }
                }
                None
            })
            .collect()
    }

    #[test]
    fn param_on_single_hydrate_emits_placeholder_and_bind() {
        let (schema, owner) = fixture_schema();
        let sel = Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::All,
            conditions: Some(Predicate::Eq(
                id_col(),
                Value::Runtime { expr: var_ref(&Symbol::from("id")), ty: ValueType::Int },
            )),
            orders: vec![],
            limit: Some(LimitSpec(1)),
            joins: vec![],
            preloads: vec![],
        };
        let table = &schema.tables[&Symbol::from("articles")];
        let body = emit_single_hydrate(&sel, table, &owner, true);

        // SQL keys on the static shape — placeholder, no inline escape call.
        let sql = prepare_sql(&body);
        assert!(sql.contains("WHERE id = ?"), "expected placeholder WHERE; got:\n{sql}");
        assert!(!sql.contains("«expr»"), "no inline value segment expected; got:\n{sql}");

        // A `Db.bind_int(stmt, 1, id)` lands right after prepare.
        let calls = db_stmt_calls(&body);
        let bind = calls.iter().find(|(m, _)| m == "bind_int").expect("bind_int call");
        assert_eq!(bind.1.len(), 3, "bind_int(stmt, idx, expr)");
        assert!(
            matches!(bind.1[1].node.as_ref(), ExprNode::Lit { value: Literal::Int { value: 1 } }),
            "1-based bind index"
        );
        // And exactly one bind for one runtime value.
        assert_eq!(calls.iter().filter(|(m, _)| m.starts_with("bind_")).count(), 1);
    }

    #[test]
    fn param_off_keeps_inline_escape() {
        // Same op, gate OFF → the pre-existing inline-escape emit: no
        // `?`, no bind call (byte-identical to today).
        let (schema, owner) = fixture_schema();
        let sel = Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::All,
            conditions: Some(Predicate::Eq(
                id_col(),
                Value::Runtime { expr: var_ref(&Symbol::from("id")), ty: ValueType::Int },
            )),
            orders: vec![],
            limit: Some(LimitSpec(1)),
            joins: vec![],
            preloads: vec![],
        };
        let table = &schema.tables[&Symbol::from("articles")];
        let body = emit_single_hydrate(&sel, table, &owner, false);

        let sql = prepare_sql(&body);
        assert!(!sql.contains('?'), "no placeholder when gate off; got:\n{sql}");
        assert!(sql.contains("«expr»"), "expected inline escape value segment; got:\n{sql}");
        assert!(
            db_stmt_calls(&body).iter().all(|(m, _)| !m.starts_with("bind_")),
            "no bind calls when gate off"
        );
        // Shape unchanged: 5-stmt Seq.
        assert_eq!(seq_len(&body), 5);
    }

    #[test]
    fn param_on_count_with_where_binds() {
        let (schema, _) = fixture_schema();
        let sel = Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::Count,
            conditions: Some(Predicate::Eq(
                id_col(),
                Value::Runtime { expr: var_ref(&Symbol::from("id")), ty: ValueType::Int },
            )),
            orders: vec![],
            limit: None,
            joins: vec![],
            preloads: vec![],
        };
        let table = &schema.tables[&Symbol::from("articles")];
        let body = emit_count(&sel, table, true);
        let sql = prepare_sql(&body);
        assert!(sql.contains("COUNT(*)") && sql.contains("WHERE id = ?"), "got:\n{sql}");
        assert!(db_stmt_calls(&body).iter().any(|(m, _)| m == "bind_int"), "expected bind_int");
    }

    #[test]
    fn param_on_string_value_binds_text() {
        // A Str-typed runtime value picks `bind_text` (the monomorphic
        // dispatch the ValueType tag already carries).
        let (schema, _) = fixture_schema();
        let sel = Select {
            table: TableRef(Symbol::from("articles")),
            columns: ColumnSpec::Exists,
            conditions: Some(Predicate::Eq(
                ColRef { table: TableRef(Symbol::from("articles")), column: Symbol::from("title") },
                Value::Runtime { expr: var_ref(&Symbol::from("title")), ty: ValueType::Str },
            )),
            orders: vec![],
            limit: Some(LimitSpec(1)),
            joins: vec![],
            preloads: vec![],
        };
        let table = &schema.tables[&Symbol::from("articles")];
        let body = emit_exists(&sel, table, true);
        assert!(prepare_sql(&body).contains("WHERE title = ?"));
        assert!(db_stmt_calls(&body).iter().any(|(m, _)| m == "bind_text"), "expected bind_text");
    }
}
