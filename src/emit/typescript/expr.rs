//! Generic TypeScript body / expression / literal emission. Used by
//! the standalone `emit_method` (runtime extraction) and indirectly by
//! controller / view / model / spec emitters that fall back to
//! arbitrary `Expr` rendering.
//!
//! Since the js_ast migration this module BUILDS a typed JS tree
//! (`js_ast::Js` / `js_ast::JsStmt`) instead of strings; the printer
//! (`printer.rs`) owns parenthesization (precedence-driven), escaping,
//! and layout. The legacy string entrypoints (`emit_expr`,
//! `emit_body`) survive as thin render wrappers until the module-level
//! emitters construct `JsModule`s directly — at which point the
//! `Span`s every node carries become token-level source-map entries.

use std::collections::{HashMap, HashSet};

use super::js_ast::{
    ArrowBody, Js, JsExpr, JsKey, JsObjEntry, JsParam, JsStmt, JsStmtNode, TplPart, TsType,
    VarKind,
};
use super::naming::{ts_field_name, ts_method_name};
use crate::expr::{desugar_op_assign, Expr, ExprNode, IrHint, LValue, Literal, RescueClause};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

// Async-name set ------------------------------------------------------
//
// Phase 3 of async coloring: the TS emitter prepends `await ` to
// Send sites whose method name is in the active deployment profile's
// async-method set. The set is thread-local because the existing
// emit pipeline is shaped as a deeply-nested call tree of pure
// functions — threading a context object through every signature
// would bloat the diff. The thread-local is set once at the public
// `emit_with_profile` entrypoint and cleared on exit; every other
// emit function reads it via `is_async_method_name` without knowing
// about it.
//
// Empty set (the default and the `node-sync` profile state) → no
// `await` ever emitted → emit byte-equivalent to pre-Phase-3
// (Gate 1).

std::thread_local! {
    static ASYNC_METHOD_NAMES: std::cell::RefCell<HashSet<Symbol>> =
        std::cell::RefCell::new(HashSet::new());
    /// Whether the body currently being emitted belongs to an
    /// `async`-marked method. The Yield emit reads this to decide
    /// between `__block(...)` (sync method body, await would be a
    /// parse error) and `(await __block(...))` (async method body,
    /// caller may pass an async block whose result must resolve
    /// before being string-interpolated).
    static IN_ASYNC_METHOD: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// Mark the current emit context as inside an async method body.
/// `library.rs::emit_method_def` wraps the body emit with this when
/// `method.is_async`. Restored on return.
pub(crate) fn with_async_method_context<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = IN_ASYNC_METHOD.with(|c| c.replace(true));
    let r = f();
    IN_ASYNC_METHOD.with(|c| c.set(prev));
    r
}

pub(super) fn in_async_method() -> bool {
    IN_ASYNC_METHOD.with(|c| c.get())
}

/// Run `f` with `names` as the active async-method set. The previous
/// value is restored on return — supports nested calls (one level of
/// nesting today; reserved for future re-entrant emit).
pub(crate) fn with_async_methods<F, R>(names: HashSet<Symbol>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = ASYNC_METHOD_NAMES.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), names));
    let r = f();
    ASYNC_METHOD_NAMES.with(|cell| *cell.borrow_mut() = prev);
    r
}

/// True iff `name` is in the active async-method set. Send-site
/// callers consult this to decide whether to wrap with `await`.
pub(crate) fn is_async_method_name(name: &str) -> bool {
    ASYNC_METHOD_NAMES.with(|cell| {
        let set = cell.borrow();
        set.iter().any(|s| s.as_str() == name)
    })
}

/// Insert additional names into the active async-method set without
/// stack-saving the previous value. The TS emit pipeline calls this
/// after global runtime+app propagation has discovered names beyond
/// the original adapter-seed extern (e.g. `save`, `destroy` on
/// `Base` propagate to async via `insert`/`update`/`delete`; user
/// model methods like `Article#comments` propagate via `where`).
/// Without these names in the set, call sites like `this.save()` or
/// `this.comments()` wouldn't get `await`-wrapped even though the
/// resolved method is async.
pub(crate) fn extend_async_methods(extra: HashSet<Symbol>) {
    ASYNC_METHOD_NAMES.with(|cell| cell.borrow_mut().extend(extra));
}

/// True iff `expr` (or any descendant) contains a `Send` whose
/// method name is in the active async set. Used by Lambda emit to
/// decide whether the lambda needs an `async` prefix and by HOF
/// emit to decide whether to rewrite a `.map`/`.each`/etc. call to
/// a `for...of` IIFE.
pub(super) fn body_has_async_send(expr: &Expr) -> bool {
    crate::analyze::async_color::expr_contains_async_send(expr, is_async_method_name)
}

// Enclosing-method parameter names ------------------------------------
//
// Mirrors the propagation pass's parameter-name filter at emit time.
// A bare `Send { recv: None, method }` whose method matches one of
// the enclosing method's parameter names is a Var read disguised as a
// Send (Ruby implicit-self resolves to the local). Without this filter
// at emit time, view-function bodies whose parameter is named `article`
// — which collides with `Views::Articles#article` (a partial-renderer
// async-marked method) — emit `(await article)` for every parameter
// reference and tsc rejects them with TS1308 (await outside async fn).
//
// Set by `with_method_params` around each method/function body emit;
// cleared on return so nested method emits don't see stale names.

std::thread_local! {
    static CURRENT_METHOD_PARAMS: std::cell::RefCell<HashSet<Symbol>> =
        std::cell::RefCell::new(HashSet::new());
}

/// Run `f` with `params` as the active enclosing-method parameter
/// name set. Stack-saves and restores so nested emits (HOF blocks,
/// rescue arms, etc.) don't drop their enclosing context.
pub(crate) fn with_method_params<F, R>(params: HashSet<Symbol>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev =
        CURRENT_METHOD_PARAMS.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), params));
    let r = f();
    CURRENT_METHOD_PARAMS.with(|cell| *cell.borrow_mut() = prev);
    r
}

/// True iff `name` matches a parameter of the currently-emitting
/// method. Used by the await-wrap site to skip wrapping bare Sends
/// (recv:None) whose name shadows an enclosing parameter.
pub(crate) fn is_enclosing_param_name(name: &str) -> bool {
    CURRENT_METHOD_PARAMS.with(|cell| {
        let set = cell.borrow();
        set.iter().any(|s| s.as_str() == name)
    })
}

// Temporal (Date/DateTime/Time) column storage split -------------------
//
// The datetime Stage-2 split: storage stays ISO-8601 TEXT in a
// `_<col>: string` backing field, and the column's reader `<col>` is a
// computed `get <col>(): Date | null` that parses the backing. So every
// internal STORAGE reference — `@col` ivar reads/writes and `x.col = v`
// writer-sends — retargets to the `_<col>` backing, while a `.col` READ
// (a zero-arg Send on some receiver) is untouched and hits the `Date`
// getter. Set per class in `js_library_class`; cleared for the
// function-shaped emits (views/jbuilder) that read model datetimes
// through the getter.

std::thread_local! {
    /// Ruby names of the current model's temporal columns (`created_at`,
    /// `updated_at`, …). Empty for non-temporal classes, Rows, views,
    /// controllers.
    static TEMPORAL_COLS: std::cell::RefCell<HashSet<String>> =
        std::cell::RefCell::new(HashSet::new());
}

/// Install the current class's temporal-column ruby names — the
/// storage/reader split. Empty for every non-temporal emit.
pub(super) fn set_temporal_cols(set: HashSet<String>) {
    TEMPORAL_COLS.with(|cell| *cell.borrow_mut() = set);
}

/// Storage field name for an ivar/attr: a temporal column stores its
/// ISO-8601 text in a `_<col>` backing (its base name is the `Date`
/// getter), so a STORAGE reference retargets there. Non-temporal names
/// pass through `ts_field_name` unchanged.
fn storage_field_name(name: &str) -> String {
    let field = ts_field_name(name);
    if TEMPORAL_COLS.with(|cell| cell.borrow().contains(name)) {
        format!("_{field}")
    } else {
        field
    }
}

/// Receiver-aware filter at emit time. Mirrors
/// `crate::analyze::async_color::recv_is_known_sync` — if the
/// receiver's resolved type is one of the known-sync containers
/// (Array/Hash/Str + framework value classes that share AR adapter
/// method names), the await wrap is suppressed. Defaults to `false`
/// (no filter) when the receiver type isn't populated, matching
/// the propagation pass.
fn recv_is_known_sync_at_emit(recv: Option<&Expr>) -> bool {
    let Some(recv) = recv else {
        return false;
    };
    // `Const` receivers (`Route.new(...)`, `MatchResult.new(...)`)
    // don't carry a `.ty` because they refer to the class itself, not
    // a value. Match the leaf segment against the known-sync set so
    // the `Class.new(...)` call site doesn't get `await`-wrapped
    // when an unrelated class's `new` is in the active async set.
    // Mirrors `analyze::async_color::recv_is_known_sync`.
    if let ExprNode::Const { path } = &*recv.node {
        if let Some(last) = path.last() {
            let name = last.as_str();
            if matches!(name, "Route" | "MatchResult") {
                return true;
            }
        }
    }
    let Some(ty) = &recv.ty else {
        return false;
    };
    match ty {
        Ty::Array { .. } | Ty::Hash { .. } | Ty::Str => true,
        Ty::Class { id, .. } => {
            let raw = id.0.as_str();
            let last = raw.rsplit("::").next().unwrap_or(raw);
            matches!(
                last,
                "ErrorCollection"
                    | "Errors"
                    | "Flash"
                    | "Session"
                    | "Parameters"
                    | "Array"
                    | "Hash"
                    | "String"
                    | "Symbol"
                    | "Route"
                    | "MatchResult"
            )
        }
        _ => false,
    }
}

// Small constructors ---------------------------------------------------

/// Synthesized identifier — emitter-invented glue with no source
/// position (`__r`, `__block`, `e`, …).
fn synth_ident(name: &str) -> Js {
    Js::synth(JsExpr::Ident(name.into()))
}

fn js_param(name: impl Into<String>) -> JsParam {
    JsParam { name: name.into(), optional: false, ty: None, default: None }
}

/// `(() => { <stmts> })()` — statement smuggled into expression
/// position. The printer derives the parens around the arrow from
/// precedence.
fn iife(span: Span, stmts: Vec<JsStmt>) -> Js {
    Js::call(
        span,
        Js::synth(JsExpr::Arrow { params: vec![], body: ArrowBody::Block(stmts), is_async: false }),
        vec![],
    )
}

/// `await (async () => { <stmts> })()` — the async-HOF rewrite shell.
/// The rewrite owns the outer await: the surrounding coloring
/// machinery doesn't know `each`/`map`/etc are now async.
fn async_iife_awaited(span: Span, stmts: Vec<JsStmt>) -> Js {
    Js::await_(
        span,
        Js::call(
            span,
            Js::synth(JsExpr::Arrow {
                params: vec![],
                body: ArrowBody::Block(stmts),
                is_async: true,
            }),
            vec![],
        ),
    )
}

/// `(() => { throw new Error("<msg>"); })()`
fn iife_throw_msg(span: Span, msg: &str) -> Js {
    let err = Js::synth(JsExpr::New {
        callee: synth_ident("Error"),
        args: vec![Js::str(Span::synthetic(), msg)],
    });
    iife(span, vec![JsStmt::synth(JsStmtNode::Throw(err))])
}

fn const_decl(name: &str, init: Js) -> JsStmt {
    JsStmt::synth(JsStmtNode::VarDecl {
        kind: VarKind::Const,
        name: name.into(),
        ty: None,
        init: Some(init),
    })
}

fn return_stmt(value: Option<Js>) -> JsStmt {
    JsStmt::synth(JsStmtNode::Return(value))
}

// Body + statements -----------------------------------------------------

pub(super) fn js_body(body: &Expr, return_ty: &Ty) -> Vec<JsStmt> {
    // Pre-walk: find local-var names assigned more than once in this
    // method body. They'll emit as `let` at first occurrence and bare
    // `name = value` thereafter. Names assigned exactly once still
    // emit as `const`. The `declared` set tracks which reassigned names
    // have already had their declaration emitted as we walk in source
    // order.
    let mut counts: HashMap<Symbol, usize> = HashMap::new();
    count_var_assignments(body, &mut counts);
    let reassigned: HashSet<Symbol> =
        counts.into_iter().filter(|(_, n)| *n > 1).map(|(s, _)| s).collect();
    let mut declared: HashSet<Symbol> = HashSet::new();
    // Names whose first assignment lives inside a nested block (an
    // `if`/`else` arm, a `case` branch, …) need a hoisted `let`
    // declaration at the function-body level — TS `let` is block-
    // scoped, so a `let x = ...` inside the if-arm doesn't reach
    // sibling statements. Without hoisting, Ruby idioms like
    //   if cond
    //     x = "..."
    //     return x
    //   end
    //   x = "..."   # second assignment, outside the if
    // emit a TS file that references `x` outside of any visible
    // declaration. Always-hoist would lose the readable
    // `let x = init;` form for top-level-first reassignments;
    // restrict to vars whose top-level assignment count is strictly
    // less than their total count — i.e., at least one assignment
    // lives in a nested branch.
    let mut top_level_counts: HashMap<Symbol, usize> = HashMap::new();
    count_top_level_var_assignments(body, &mut top_level_counts);
    let mut hoisted: Vec<Symbol> = reassigned
        .iter()
        .filter(|name| {
            let total = count_var_for(body, name);
            let top = top_level_counts.get(*name).copied().unwrap_or(0);
            top < total
        })
        .cloned()
        .collect();
    hoisted.sort();
    let mut stmts: Vec<JsStmt> = Vec::new();
    for name in &hoisted {
        stmts.push(JsStmt::synth(JsStmtNode::VarDecl {
            kind: VarKind::Let,
            name: escape_reserved_word(name.as_str()),
            ty: Some(TsType("any".into())),
            init: None,
        }));
        declared.insert(name.clone());
    }
    stmts.extend(js_body_with_state(body, return_ty, &reassigned, &mut declared));
    stmts
}

/// Count Var-assignment occurrences only at the top level of a
/// function body's `Seq` — siblings of the body's outermost
/// statement list. Nested branches (if-arms, case bodies) are NOT
/// counted; this lets the hoist-detection logic identify vars whose
/// reassignment splits across scopes.
fn count_top_level_var_assignments(body: &Expr, out: &mut HashMap<Symbol, usize>) {
    match &*body.node {
        ExprNode::Seq { exprs } => {
            for e in exprs {
                if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*e.node {
                    *out.entry(name.clone()).or_insert(0) += 1;
                }
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => {
            *out.entry(name.clone()).or_insert(0) += 1;
        }
        _ => {}
    }
}

/// Total count of Var-assignments to `name` in `body` (recursive).
fn count_var_for(body: &Expr, name: &Symbol) -> usize {
    let mut all: HashMap<Symbol, usize> = HashMap::new();
    count_var_assignments(body, &mut all);
    all.get(name).copied().unwrap_or(0)
}

fn js_body_with_state(
    body: &Expr,
    return_ty: &Ty,
    reassigned: &HashSet<Symbol>,
    declared: &mut HashSet<Symbol>,
) -> Vec<JsStmt> {
    let is_void = matches!(return_ty, Ty::Nil);
    match &*body.node {
        // Guard-clause: ingest rewrites `return if cond; rest...` to
        // `If { cond, then: nil, else: <rest> }` (see ingest/expr.rs's
        // "Guard-clause rewrite"). Reverse it on the way out so we
        // emit `if (cond) { return; } <rest>` instead of nesting the
        // whole method body inside the else branch. Only applies when
        // the then branch is the literal nil placeholder the rewrite
        // synthesizes.
        ExprNode::If { cond, then_branch, else_branch }
            if matches!(&*then_branch.node, ExprNode::Lit { value: Literal::Nil })
                && !is_nil_or_empty(else_branch) =>
        {
            // Ruby's `return nil` returns nil, not undefined — emit
            // `return null;` when the method has a value.
            let ret = JsStmt::new(
                then_branch.span,
                JsStmtNode::Return(if is_void { None } else { Some(Js::synth(JsExpr::Null)) }),
            );
            let mut out = vec![JsStmt::new(
                body.span,
                JsStmtNode::If { cond: js_expr(cond), then: vec![ret], else_: None },
            )];
            out.extend(js_body_with_state(else_branch, return_ty, reassigned, declared));
            out
        }
        // `def initialize(owner); @owner = owner; end` — the assignment
        // is the whole body. Emit the assignment as a statement, then
        // return its value if non-void. Without this, the side-effect
        // of setting the ivar is lost (the value alone reads the local
        // but doesn't write the ivar).
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            let assign = Js::new(
                body.span,
                JsExpr::Assign {
                    target: Js::member(
                        body.span,
                        Js::ident(body.span, "this"),
                        storage_field_name(name.as_str()),
                    ),
                    op: "=",
                    value: js_expr(value),
                },
            );
            if is_void {
                vec![JsStmt::expr(assign)]
            } else {
                vec![JsStmt::new(body.span, JsStmtNode::Return(Some(assign)))]
            }
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut out = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                out.extend(js_stmts_with_state(
                    e,
                    i == exprs.len() - 1,
                    is_void,
                    reassigned,
                    declared,
                ));
            }
            out
        }
        // Method-body-level begin/rescue emits as native try/catch rather
        // than IIFE-wrapped. Preserves control flow: early `return` inside
        // the body actually exits the method, `throw e` outside the match
        // arms rethrows cleanly, and no needless `(() => { ... })()` noise.
        ExprNode::BeginRescue { body: inner, rescues, else_branch, ensure, .. } => {
            vec![js_begin_rescue_stmt(
                body.span,
                inner,
                rescues,
                else_branch.as_ref(),
                ensure.as_ref(),
                return_ty,
            )]
        }
        // Single-Case-as-whole-body (e.g., `process_action`'s synthesized
        // dispatcher): route through the statement walker so it emits as
        // a `switch` rather than falling to the default arm.
        ExprNode::Case { .. } => js_stmts_with_state(body, true, is_void, reassigned, declared),
        _ => vec![default_stmt(body, true, is_void)],
    }
}

/// Walk an expression tree counting `Assign { LValue::Var { name } }`
/// occurrences per name. Used to identify locals that need `let`
/// declarations (mutated more than once) versus locals that fit
/// `const` (single-assignment). The traversal visits all children so
/// reassignments inside nested if/while/case branches are counted.
fn count_var_assignments(e: &Expr, out: &mut HashMap<Symbol, usize>) {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
        | ExprNode::OpAssign { target: LValue::Var { name, .. }, value, .. } => {
            *out.entry(name.clone()).or_insert(0) += 1;
            count_var_assignments(value, out);
        }
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
            count_var_assignments(value, out)
        }
        // Buffer-accumulate `var << X` is rewritten by the statement
        // walker to `var += X;` — i.e., an assignment for declaration
        // purposes. Count it so the var gets `let` (mutable) instead
        // of `const`.
        //
        // Exception: when the Send is tagged `StringBuilderAppend`,
        // the emit form is `var.push(X)` against a `string[]` array,
        // which is a method call, not a reassignment. The array
        // binding itself is single-assignment (the Init synthesizes
        // it once), so the local can stay `const`. Skip the count
        // here so the `let`/`const` decision matches the actual emit.
        ExprNode::Send { recv: Some(recv), method, args, block, .. }
            if method.as_str() == "<<" && args.len() == 1 =>
        {
            let is_string_builder_append = matches!(e.hint, Some(IrHint::StringBuilderAppend));
            if !is_string_builder_append {
                if let ExprNode::Var { name, .. } = &*recv.node {
                    *out.entry(name.clone()).or_insert(0) += 1;
                }
            }
            count_var_assignments(recv, out);
            for a in args {
                count_var_assignments(a, out);
            }
            if let Some(b) = block {
                count_var_assignments(b, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                count_var_assignments(e, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            count_var_assignments(cond, out);
            count_var_assignments(then_branch, out);
            count_var_assignments(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            count_var_assignments(left, out);
            count_var_assignments(right, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                count_var_assignments(r, out);
            }
            for a in args {
                count_var_assignments(a, out);
            }
            if let Some(b) = block {
                count_var_assignments(b, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            count_var_assignments(fun, out);
            for a in args {
                count_var_assignments(a, out);
            }
            if let Some(b) = block {
                count_var_assignments(b, out);
            }
        }
        ExprNode::While { cond, body, .. } => {
            count_var_assignments(cond, out);
            count_var_assignments(body, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            count_var_assignments(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    count_var_assignments(g, out);
                }
                count_var_assignments(&arm.body, out);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            count_var_assignments(body, out);
            for r in rescues {
                count_var_assignments(&r.body, out);
            }
            if let Some(e) = else_branch {
                count_var_assignments(e, out);
            }
            if let Some(e) = ensure {
                count_var_assignments(e, out);
            }
        }
        ExprNode::Lambda { body, .. } => count_var_assignments(body, out),
        ExprNode::Let { value, body, .. } => {
            count_var_assignments(value, out);
            count_var_assignments(body, out);
        }
        ExprNode::Return { value }
        | ExprNode::Raise { value }
        | ExprNode::RescueModifier { expr: value, .. } => count_var_assignments(value, out),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    count_var_assignments(expr, out);
                }
            }
        }
        _ => {}
    }
}

/// Render a begin/rescue at statement position — inside a method body
/// rather than as an expression. Preserves native TS control flow:
/// `try { ... } catch (e) { ... } finally { ... }` with early-return
/// and rethrow working as Ruby's semantics expect.
fn js_begin_rescue_stmt(
    span: Span,
    body: &Expr,
    rescues: &[RescueClause],
    else_branch: Option<&Expr>,
    ensure: Option<&Expr>,
    return_ty: &Ty,
) -> JsStmt {
    let mut try_body = js_body(body, return_ty);
    if let Some(eb) = else_branch {
        // Ruby's `else` runs iff the body raised nothing. Appending to
        // the try block preserves that ordering.
        try_body.extend(js_body(eb, return_ty));
    }
    let catch = catch_chain(rescues, &|rc| {
        let mut v = rescue_binding_alias(rc);
        v.extend(js_body(&rc.body, return_ty));
        v
    });
    let finally = ensure.map(|en| js_body(en, &Ty::Nil));
    JsStmt::new(
        span,
        JsStmtNode::Try { body: try_body, catch: Some((Some("e".into()), catch)), finally },
    )
}

/// Rescue binding (`rescue Err => name`) — alias the catch variable
/// `e` to the source-named binding so the rescue body's references
/// resolve.
fn rescue_binding_alias(rc: &RescueClause) -> Vec<JsStmt> {
    match &rc.binding {
        Some(name) => vec![const_decl(name.as_str(), synth_ident("e"))],
        None => vec![],
    }
}

/// Chain rescue clauses inside a `catch (e)` body as
/// `if (e instanceof X) { ... } else if ... else { throw e; }`.
/// Bare rescue (no classes) is the catchall; clauses after a bare
/// one are unreachable in Ruby too. No rescues at all (ensure-only
/// begin) still rethrows to preserve exception propagation.
fn catch_chain(rescues: &[RescueClause], clause_body: &dyn Fn(&RescueClause) -> Vec<JsStmt>) -> Vec<JsStmt> {
    let Some((rc, rest)) = rescues.split_first() else {
        return vec![JsStmt::synth(JsStmtNode::Throw(synth_ident("e")))];
    };
    if rc.classes.is_empty() {
        return clause_body(rc);
    }
    let cond = rc
        .classes
        .iter()
        .map(|c| Js::binary(c.span, "instanceof", synth_ident("e"), js_expr(c)))
        .reduce(|a, b| Js::binary(Span::synthetic(), "||", a, b))
        .expect("non-empty classes");
    vec![JsStmt::synth(JsStmtNode::If {
        cond,
        then: clause_body(rc),
        else_: Some(catch_chain(rest, clause_body)),
    })]
}

/// Pre-walk a body to identify reassigned local-variable names.
/// The result feeds `js_stmts_with_state` so multi-statement bodies
/// emit `let` (mutable) for names assigned more than once and
/// `const` for names assigned exactly once.
pub(super) fn collect_reassigned(body: &Expr) -> HashSet<Symbol> {
    let mut counts: HashMap<Symbol, usize> = HashMap::new();
    count_var_assignments(body, &mut counts);
    counts.into_iter().filter(|(_, n)| *n > 1).map(|(s, _)| s).collect()
}

/// String-accumulator hint consumer at statement position. Init and
/// Append are statement-shapes (Result lives in `js_expr`).
/// V8 specializes `[]`+`.push(...)`+`.join("")` better than repeated
/// string concat — measured 1.4-1.8× lift on HTML view bodies per
/// the bench in #18.
///
/// - `Init` rewrites the synthesized `io = String.new` Assign to
///   `const <name>: string[] = [];` and records the binding so
///   subsequent statements don't re-declare it. The
///   `count_var_assignments` pre-pass skips Append-hinted shovels,
///   so `<name>` correctly lands in single-assignment territory
///   and `const` is the right choice.
/// - `Append` rewrites the `<name> << <arg>` Send to
///   `<name>.push(<arg>);` — never wrapped in `return` (the Result
///   site is the function's tail; an Append at the tail would be
///   a lowerer bug).
fn try_string_builder_stmt(e: &Expr, declared: &mut HashSet<Symbol>) -> Option<Vec<JsStmt>> {
    match e.hint? {
        IrHint::StringBuilderInit => {
            if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*e.node {
                let escaped = escape_reserved_word(name.as_str());
                declared.insert(name.clone());
                return Some(vec![JsStmt::new(
                    e.span,
                    JsStmtNode::VarDecl {
                        kind: VarKind::Const,
                        name: escaped,
                        ty: Some(TsType("string[]".into())),
                        init: Some(Js::synth(JsExpr::Array(vec![]))),
                    },
                )]);
            }
            None
        }
        IrHint::StringBuilderAppend => {
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node {
                if method.as_str() == "<<" && args.len() == 1 {
                    if let ExprNode::Var { name, .. } = &*recv.node {
                        let buf = Js::ident(recv.span, escape_reserved_word(name.as_str()));
                        return Some(vec![JsStmt::expr(Js::method_call(
                            e.span,
                            buf,
                            "push",
                            vec![js_expr(&args[0])],
                        ))]);
                    }
                }
            }
            None
        }
        IrHint::StringBuilderResult => None, // handled in js_expr
    }
}

fn default_stmt(e: &Expr, is_last: bool, void_return: bool) -> JsStmt {
    if is_last && !void_return {
        JsStmt::new(e.span, JsStmtNode::Return(Some(js_expr(e))))
    } else {
        JsStmt::expr(js_expr(e))
    }
}

pub(super) fn js_stmts_with_state(
    e: &Expr,
    is_last: bool,
    void_return: bool,
    reassigned: &HashSet<Symbol>,
    declared: &mut HashSet<Symbol>,
) -> Vec<JsStmt> {
    // IrHint::StringBuilder{Init,Append} — lowerer-tagged accumulator
    // pattern. Init/Append are statement-position; Result is handled
    // in `js_expr` (where the terminal Var read lives).
    if let Some(stmts) = try_string_builder_stmt(e, declared) {
        return stmts;
    }
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            // First occurrence of a name that we know will be reassigned
            // → `let`. First occurrence of a name assigned exactly once
            // → `const`. Subsequent occurrences (only possible for
            // reassigned names) → bare `name = value`.
            let escaped = escape_reserved_word(name.as_str());
            let node = if reassigned.contains(name) {
                if declared.insert(name.clone()) {
                    JsStmtNode::VarDecl {
                        kind: VarKind::Let,
                        name: escaped,
                        ty: None,
                        init: Some(js_expr(value)),
                    }
                } else {
                    JsStmtNode::Expr(Js::new(
                        e.span,
                        JsExpr::Assign {
                            target: Js::ident(e.span, escaped),
                            op: "=",
                            value: js_expr(value),
                        },
                    ))
                }
            } else {
                JsStmtNode::VarDecl {
                    kind: VarKind::Const,
                    name: escaped,
                    ty: None,
                    init: Some(js_expr(value)),
                }
            };
            vec![JsStmt::new(e.span, node)]
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            vec![JsStmt::expr(Js::new(
                e.span,
                JsExpr::Assign {
                    target: Js::member(
                        e.span,
                        Js::ident(e.span, "this"),
                        storage_field_name(name.as_str()),
                    ),
                    op: "=",
                    value: js_expr(value),
                },
            ))]
        }
        // Buffer-accumulate idiom at statement position:
        // `buf << X` (Ruby) → `buf += X;` (TS), where `buf` is any
        // String-typed (or untyped) local-variable receiver. The
        // lowered view body uses this shape; form_with's inner
        // capture uses `body << ...` with a different name. Arrays
        // fall through to `js_expr` so the type-aware `<<` dispatch
        // produces `.push(...)`.
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "<<"
                && args.len() == 1
                && matches!(&*recv.node, ExprNode::Var { .. })
                && matches!(recv.ty, Some(Ty::Str) | None) =>
        {
            let ExprNode::Var { name, .. } = &*recv.node else { unreachable!() };
            vec![JsStmt::expr(Js::new(
                e.span,
                JsExpr::Assign {
                    target: Js::ident(recv.span, escape_reserved_word(name.as_str())),
                    op: "+=",
                    value: js_expr(&args[0]),
                },
            ))]
        }
        // Nested `Seq` at statement position — flatten it. A Seq can
        // reach here as the last element of an enclosing Seq (e.g. the
        // cache-aware has_many reader — issue #27). Recurse per child so
        // each statement gets proper `let`/`const`/`return` treatment;
        // only the final child inherits this Seq's `is_last`/void flags.
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut out = Vec::with_capacity(exprs.len());
            for (i, inner) in exprs.iter().enumerate() {
                out.extend(js_stmts_with_state(
                    inner,
                    is_last && i == exprs.len() - 1,
                    void_return,
                    reassigned,
                    declared,
                ));
            }
            out
        }
        // Return at statement position: emit as a native `return`
        // rather than wrapping in an IIFE. Ruby's `return nil` returns
        // nil, not undefined — emit `return null;` (not bare `return;`)
        // to preserve that semantic under TS's strict equality rules.
        ExprNode::Return { value } => {
            vec![JsStmt::new(e.span, JsStmtNode::Return(Some(js_expr(value))))]
        }
        // Guard-return pattern: `if (cond) { return X; }` at statement
        // position, with no else branch (or an else that's nil). Rather
        // than emit a ternary, produce a native guard — preserves the
        // Ruby idiom `return nil if cond` as idiomatic TS.
        ExprNode::If { cond, then_branch, else_branch }
            if matches!(&*then_branch.node, ExprNode::Return { .. })
                && is_nil_or_empty(else_branch) =>
        {
            let ExprNode::Return { value } = &*then_branch.node else { unreachable!() };
            vec![JsStmt::new(
                e.span,
                JsStmtNode::If {
                    cond: js_expr(cond),
                    then: vec![JsStmt::new(
                        then_branch.span,
                        JsStmtNode::Return(Some(js_expr(value))),
                    )],
                    else_: None,
                },
            )]
        }
        // Postfix-`if` at statement position with no else branch.
        // Ruby's `x = [] if x.nil?` lowers to `If { cond, then=Assign,
        // else=nil }`. A ternary would drop the assignment's LHS
        // (`Assign` in expression position emits only the rhs). Emit
        // a native `if (cond) { <stmt> }` instead — preserves the side
        // effect.
        ExprNode::If { cond, then_branch, else_branch } if is_nil_or_empty(else_branch) => {
            vec![JsStmt::new(
                e.span,
                JsStmtNode::If {
                    cond: js_expr(cond),
                    then: js_branch(then_branch, reassigned, declared),
                    else_: None,
                },
            )]
        }
        // Two-branch (or chained-elsif) `if` at statement position
        // when the value isn't being returned. A ternary (correct for
        // value-position) would discard side effects of mutating
        // branches; block-form `if/else` preserves them. When
        // `is_last && !void_return`, fall through to ternary so the
        // value still flows out.
        ExprNode::If { cond, then_branch, else_branch } if !is_last || void_return => {
            vec![js_if_chain(e.span, cond, then_branch, else_branch, reassigned, declared)]
        }
        // `while cond; body; end` and `until cond; body; end` at
        // statement position emit as native loops. The until form
        // negates the condition (TS has no `until` keyword).
        ExprNode::While { cond, body, until_form } => {
            let cond_js = if *until_form {
                Js::unary(cond.span, "!", js_expr(cond))
            } else {
                js_expr(cond)
            };
            vec![JsStmt::new(
                e.span,
                JsStmtNode::While { cond: cond_js, body: js_branch(body, reassigned, declared) },
            )]
        }
        // `next` inside a Ruby block lowers to `return` from the JS
        // callback (since blocks become arrow functions). `next` with
        // a value (rare) returns that value; bare `next` returns
        // undefined. The synthesized lambda carries no value out, so
        // bare-return is fine.
        ExprNode::Next { value } => {
            vec![JsStmt::new(e.span, JsStmtNode::Return(value.as_ref().map(|v| js_expr(v))))]
        }
        // `case scrutinee; when X then body; ...; end` at statement
        // position. Emit as a TS `switch` when every arm pattern is a
        // single literal and the scrutinee is a simple value. Falls
        // through to the default-arm rendering for non-literal
        // patterns — the `process_action` dispatcher (the only
        // producer here today) always uses literal-symbol arms.
        ExprNode::Case { scrutinee, arms }
            if arms.iter().all(|a| {
                a.guard.is_none() && matches!(&a.pattern, crate::expr::Pattern::Lit { .. })
            }) =>
        {
            let cases = arms
                .iter()
                .map(|arm| {
                    let pat = match &arm.pattern {
                        crate::expr::Pattern::Lit { value } => {
                            js_literal(Span::synthetic(), value)
                        }
                        _ => unreachable!(),
                    };
                    (pat, js_stmts_with_state(&arm.body, false, true, reassigned, declared))
                })
                .collect();
            vec![JsStmt::new(
                e.span,
                JsStmtNode::Switch { scrutinee: js_expr(scrutinee), cases, default: None },
            )]
        }
        _ => vec![default_stmt(e, is_last, void_return)],
    }
}

/// Build a statement-position `if/else if/.../else` chain. The
/// printer renders a `Some([If])` else-branch as a flat `else if`.
fn js_if_chain(
    span: Span,
    cond: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    reassigned: &HashSet<Symbol>,
    declared: &mut HashSet<Symbol>,
) -> JsStmt {
    let then = js_branch(then_branch, reassigned, declared);
    let else_ = if is_nil_or_empty(else_branch) {
        None
    } else if let ExprNode::If { cond, then_branch, else_branch: inner_else } = &*else_branch.node
    {
        Some(vec![js_if_chain(
            else_branch.span,
            cond,
            then_branch,
            inner_else,
            reassigned,
            declared,
        )])
    } else {
        Some(js_branch(else_branch, reassigned, declared))
    };
    JsStmt::new(span, JsStmtNode::If { cond: js_expr(cond), then, else_ })
}

/// Statement list for a single branch of an `if`/`while` block.
/// Branches are statements (no implicit return), so void_return =
/// true; a `Seq` flattens so each child gets proper stmt treatment.
fn js_branch(
    e: &Expr,
    reassigned: &HashSet<Symbol>,
    declared: &mut HashSet<Symbol>,
) -> Vec<JsStmt> {
    match &*e.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut out = Vec::new();
            for (i, sub) in exprs.iter().enumerate() {
                out.extend(js_stmts_with_state(
                    sub,
                    i == exprs.len() - 1,
                    true,
                    reassigned,
                    declared,
                ));
            }
            out
        }
        _ => js_stmts_with_state(e, true, true, reassigned, declared),
    }
}

/// Statement list from an expression in a context where any locals
/// belong to the ENCLOSING scope: each `Seq` member becomes a bare
/// expression statement (assignments stay `name = value`, no
/// declaration). Used by the expression-position IIFE shells
/// (while-as-expression, async HOF bodies) whose Ruby source closes
/// over the surrounding method's locals.
fn expr_stmts(body: &Expr) -> Vec<JsStmt> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().map(|x| JsStmt::expr(js_expr(x))).collect(),
        _ => vec![JsStmt::expr(js_expr(body))],
    }
}

/// Suffix `_` to JS reserved words used as identifiers. Mirrors the
/// `escape_reserved` in the parent module that's applied to method
/// parameter names; here we apply it to local-variable references so
/// `params.fetch(:k, default)`'s body sees `default_` (matching the
/// param-name escape) instead of bare `default` (a JS keyword).
fn escape_reserved_word(name: &str) -> String {
    matches!(
        name,
        "default"
            | "with"
            | "function"
            | "class"
            | "for"
            | "let"
            | "const"
            | "var"
            | "return"
            | "switch"
            | "case"
            | "if"
            | "else"
            | "while"
            | "do"
            | "yield"
            | "delete"
            | "new"
            | "this"
            | "super"
            | "true"
            | "false"
            | "null"
            | "void"
            | "typeof"
            | "instanceof"
    )
    .then(|| format!("{name}_"))
    .unwrap_or_else(|| name.to_string())
}

/// Unwrap a `Union<T, Nil>` to `T` for type-aware dispatch. The
/// flow-sensitive ivar typer wraps every ivar's type in
/// `Union<T, Nil>` because a first read can observe nil before any
/// assignment runs (see `parse_library_with_rbs`'s flow_ivars
/// reseed). The actual value is still `T` everywhere except the
/// possibly-nil first-read window, so dispatch on `T` is correct
/// for emit purposes. `Union<Nil>` and other shapes pass through
/// unchanged.
fn strip_nullable(ty: Option<&Ty>) -> Option<&Ty> {
    let ty = ty?;
    if let Ty::Union { variants } = ty {
        if variants.len() == 2 {
            let nil_idx = variants.iter().position(|v| matches!(v, Ty::Nil));
            if let Some(idx) = nil_idx {
                return Some(&variants[1 - idx]);
            }
        }
    }
    Some(ty)
}

fn is_nil_or_empty(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Lit { value: Literal::Nil } => true,
        ExprNode::Seq { exprs } => exprs.is_empty(),
        _ => false,
    }
}

// Expressions -----------------------------------------------------------

/// Legacy string entrypoint: render one expression. The tree is
/// rendered at lowest precedence — callers splice the result into
/// full-expression slots (initializers, parameter defaults).
pub(super) fn emit_expr(e: &Expr) -> String {
    super::printer::render_expr(&js_expr(e))
}

pub(super) fn js_expr(e: &Expr) -> Js {
    // Analyzer-set diagnostic annotations short-circuit to a target
    // raise-equivalent (preserves Ruby's runtime-raise semantics).
    if let Some(kind) = &e.diagnostic {
        return Js::new(
            e.span,
            JsExpr::Raw(
                crate::emit::diagnostics::StubStyle::TsThrow
                    .render(&crate::diagnostic::Diagnostic::stub_text(kind)),
            ),
        );
    }
    // IrHint::StringBuilder* — expression-position consumers.
    // Init/Append at statement position are handled in
    // `js_stmts_with_state` first; this branch catches
    // Append-inside-lambda (`articles.forEach(a => io << render(a))`
    // — the partial lowerer's collection-render shape) and the
    // terminal Result Var read at the function tail.
    match e.hint {
        Some(IrHint::StringBuilderAppend) => {
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node {
                if method.as_str() == "<<" && args.len() == 1 {
                    if let ExprNode::Var { name, .. } = &*recv.node {
                        let buf = Js::ident(recv.span, escape_reserved_word(name.as_str()));
                        return Js::method_call(e.span, buf, "push", vec![js_expr(&args[0])]);
                    }
                }
            }
        }
        Some(IrHint::StringBuilderResult) => {
            if let ExprNode::Var { name, .. } = &*e.node {
                let buf = Js::ident(e.span, escape_reserved_word(name.as_str()));
                return Js::method_call(
                    e.span,
                    buf,
                    "join",
                    vec![Js::str(Span::synthetic(), "")],
                );
            }
        }
        _ => {}
    }
    match &*e.node {
        ExprNode::Lit { value } => js_literal(e.span, value),
        ExprNode::Const { path } => Js::ident(e.span, const_name(path)),
        ExprNode::Var { name, .. } => Js::ident(e.span, escape_reserved_word(name.as_str())),
        ExprNode::Ivar { name } => Js::member(
            e.span,
            Js::ident(e.span, "this"),
            storage_field_name(name.as_str()),
        ),
        ExprNode::Send { recv, method, args, block, parenthesized } => js_send_with_block(
            e.span,
            recv.as_ref(),
            method.as_str(),
            args,
            block.as_ref(),
            *parenthesized,
        ),
        ExprNode::Assign { target, value } => {
            // Expression-position assignment — preserve the side effect.
            // JS `a = b` is an expression that both assigns and yields
            // the value.
            let value_js = js_expr(value);
            let target_js = match target {
                LValue::Var { name, .. } => {
                    Js::ident(e.span, escape_reserved_word(name.as_str()))
                }
                LValue::Ivar { name } => Js::member(
                    e.span,
                    Js::ident(e.span, "this"),
                    storage_field_name(name.as_str()),
                ),
                LValue::Attr { recv, name } => {
                    // A temporal-column write (`x.created_at = v`) targets
                    // the `_<col>` string backing (the base name is the
                    // read-only `Date` getter); non-temporal names pass
                    // through unchanged.
                    Js::member(e.span, js_expr(recv), storage_field_name(name.as_str()))
                }
                LValue::Index { recv, index } => Js::index(e.span, js_expr(recv), js_expr(index)),
                _ => return value_js,
            };
            Js::new(e.span, JsExpr::Assign { target: target_js, op: "=", value: value_js })
        }
        // Expression-position statement list. An IIFE evaluates the
        // members in order and yields the last — the legacy `a; b`
        // splice was a syntax error in any real expression slot.
        ExprNode::Seq { exprs } => match exprs.as_slice() {
            [] => Js::new(e.span, JsExpr::Null),
            [only] => js_expr(only),
            many => {
                let mut stmts: Vec<JsStmt> = Vec::with_capacity(many.len());
                for x in &many[..many.len() - 1] {
                    stmts.push(JsStmt::expr(js_expr(x)));
                }
                stmts.push(return_stmt(Some(js_expr(&many[many.len() - 1]))));
                iife(e.span, stmts)
            }
        },
        ExprNode::If { cond, then_branch, else_branch } => Js::new(
            e.span,
            JsExpr::Ternary {
                cond: js_expr(cond),
                then: js_expr(then_branch),
                else_: js_expr(else_branch),
            },
        ),
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            Js::binary(e.span, op_s, js_expr(left), js_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            Js::new(e.span, JsExpr::Array(elements.iter().map(js_expr).collect()))
        }
        ExprNode::Hash { entries, .. } => Js::new(
            e.span,
            JsExpr::Object(
                entries
                    .iter()
                    .map(|(k, v)| JsObjEntry::Prop(js_obj_key(k), js_expr(v)))
                    .collect(),
            ),
        ),
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let tpl = parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => TplPart::Text(value.clone()),
                    InterpPart::Expr { expr } => TplPart::Expr(js_expr(expr)),
                })
                .collect();
            Js::new(e.span, JsExpr::Template(tpl))
        }
        ExprNode::SelfRef => Js::ident(e.span, "this"),
        ExprNode::Lambda { params, body, .. } => {
            // Async coloring (Phase 3): a Lambda whose body contains
            // a Send to a name in the active async set must itself
            // be `async`, otherwise the inner `await` emit sites are
            // syntax errors.
            let body_async = body_has_async_send(body);
            let js_params: Vec<JsParam> =
                params.iter().map(|p| js_param(p.as_str())).collect();
            // Multi-statement bodies need a block form so each
            // statement separates cleanly. Single-expression bodies
            // stay in the concise `args => expr` form. Lambdas open a
            // fresh scope: pre-walk the body to identify reassigned
            // locals (so e.g. an inner capture buffer `body =
            // String.new` followed by `body << X` emits as `let body`
            // not `const body`).
            let arrow_body = match &*body.node {
                ExprNode::Seq { exprs } if exprs.len() > 1 => {
                    let reassigned = collect_reassigned(body);
                    let mut declared: HashSet<Symbol> = HashSet::new();
                    let mut stmts = Vec::new();
                    for (i, x) in exprs.iter().enumerate() {
                        stmts.extend(js_stmts_with_state(
                            x,
                            i == exprs.len() - 1,
                            false,
                            &reassigned,
                            &mut declared,
                        ));
                    }
                    ArrowBody::Block(stmts)
                }
                _ => ArrowBody::Expr(js_expr(body)),
            };
            Js::new(
                e.span,
                JsExpr::Arrow { params: js_params, body: arrow_body, is_async: body_async },
            )
        }
        ExprNode::Return { value } => {
            // Expression-position return is rare — typically the
            // statement-level walker handles Return cleanly. An IIFE
            // preserves semantics when Return appears inside a larger
            // expression.
            iife(e.span, vec![return_stmt(Some(js_expr(value)))])
        }
        ExprNode::Super { args } => {
            // Ruby's `super` forwards to the parent class's same-named
            // method. TS requires `super.methodName(...)`, which needs
            // enclosing-method context that this emitter doesn't carry.
            // Emit syntactically-valid `super(...)` — class-level
            // emitters rewrite the IR to `super.X(...)` where they
            // know X.
            let args_js = match args {
                None => vec![],
                Some(a) => a.iter().map(js_expr).collect(),
            };
            Js::call(e.span, Js::ident(e.span, "super"), args_js)
        }
        ExprNode::BeginRescue { body, rescues, ensure, .. } => {
            // Expression-position begin/rescue — wrap the try/catch in
            // an IIFE so the whole thing evaluates to a value. Seq
            // bodies render as statements with the last expression
            // returned; locals stay enclosing-scoped (bare
            // assignments), matching Ruby's begin-block semantics.
            let mut try_body: Vec<JsStmt> = Vec::new();
            match &*body.node {
                ExprNode::Seq { exprs } if !exprs.is_empty() => {
                    for x in &exprs[..exprs.len() - 1] {
                        try_body.push(JsStmt::expr(js_expr(x)));
                    }
                    try_body.push(return_stmt(Some(js_expr(&exprs[exprs.len() - 1]))));
                }
                _ => try_body.push(return_stmt(Some(js_expr(body)))),
            }
            let catch = catch_chain(rescues, &|rc| {
                let mut v = rescue_binding_alias(rc);
                v.push(return_stmt(Some(js_expr(&rc.body))));
                v
            });
            let finally = ensure.as_ref().map(|en| vec![JsStmt::expr(js_expr(en))]);
            iife(
                e.span,
                vec![JsStmt::new(
                    e.span,
                    JsStmtNode::Try {
                        body: try_body,
                        catch: Some((Some("e".into()), catch)),
                        finally,
                    },
                )],
            )
        }
        ExprNode::RescueModifier { expr, fallback } => iife(
            e.span,
            vec![JsStmt::new(
                e.span,
                JsStmtNode::Try {
                    body: vec![return_stmt(Some(js_expr(expr)))],
                    catch: Some((None, vec![return_stmt(Some(js_expr(fallback)))])),
                    finally: None,
                },
            )],
        ),
        ExprNode::Yield { args } => {
            // Ruby's `yield` invokes the enclosing method's implicit
            // block. Library-class emit gives every yield-using method
            // an injected `__block` parameter (see emit_plain_method);
            // here we just call it. Awaited only when the enclosing
            // method is async (the colorer marks methods that capture
            // yield's return into a binding, like `body =
            // yield(builder)` in form_with). Sync methods that
            // yield-without-capture get a plain call; awaiting there
            // would be a parse error since the function isn't async.
            let call = Js::call(
                e.span,
                Js::ident(e.span, "__block"),
                args.iter().map(js_expr).collect(),
            );
            if in_async_method() {
                Js::await_(e.span, call)
            } else {
                call
            }
        }
        ExprNode::While { cond, body, until_form } => {
            // `while`/`until` at expression position is unusual —
            // wrap in IIFE so the syntactic position works. Statement-
            // position uses are handled in `js_stmts_with_state`.
            let cond_js = if *until_form {
                Js::unary(cond.span, "!", js_expr(cond))
            } else {
                js_expr(cond)
            };
            iife(
                e.span,
                vec![JsStmt::synth(JsStmtNode::While {
                    cond: cond_js,
                    body: expr_stmts(body),
                })],
            )
        }
        ExprNode::Next { value } => {
            iife(e.span, vec![return_stmt(value.as_ref().map(|v| js_expr(v)))])
        }
        ExprNode::Cast { value, target_ty } => {
            // TS `as T` is a compile-time assertion — no runtime
            // narrowing. The lowerer adds Cast at adapter-row sites
            // where the static value type is wider than the column;
            // TS's `any` lets the assignment through without `as`,
            // but emitting it documents intent and helps TS's narrowing.
            Js::new(
                e.span,
                JsExpr::Cast { expr: js_expr(value), ty: TsType(super::ty::ts_ty(target_ty)) },
            )
        }
        ExprNode::Raise { value } => {
            // `throw` is a statement in JS/TS, but expressions appear
            // in ternary arms and the last-expression-of-method slot.
            // Wrap in an IIFE so the same emit form works at both
            // statement and expression position.
            iife(e.span, vec![JsStmt::synth(JsStmtNode::Throw(js_expr(value)))])
        }
        ExprNode::OpAssign { target, op, value } => {
            // Desugar `target op= value` to the existing-IR shape and
            // recurse. TS has native compound assignment, but the
            // desugared form routes back through the Assign + Send/If
            // arms above without duplicating LValue dispatch.
            js_expr(&desugar_op_assign(target, *op, value, e.span))
        }
        other => Js::new(
            e.span,
            JsExpr::Raw(crate::emit::diagnostics::report_unsupported(
                e.span,
                "typescript",
                other.kind_str(),
                "",
            )),
        ),
    }
}

/// Send emission that folds a trailing block (Ruby `do ... end` /
/// `&:sym`) in as an arrow-function argument — TS's closest
/// equivalent. The block-less path delegates directly to `js_send`.
fn js_send_with_block(
    span: Span,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    parenthesized: bool,
) -> Js {
    let Some(blk) = block else {
        return js_send(span, recv, method, args, parenthesized);
    };
    // `N.times { |i| body }` — JS Numbers have no `.times` method,
    // so a fall-through emit produces `3.times(i => …)` which tsc
    // (and esbuild's parser) reject. Rewrite to a counted loop that
    // invokes the block as a lambda each iteration. Sync bodies stay
    // sync; the async-HOF rewrite below handles async-body cases.
    if method == "times" && args.is_empty() && recv.is_some() {
        if let ExprNode::Lambda { body, .. } = &*blk.node {
            if !body_has_async_send(body) {
                let stmts = vec![
                    const_decl("__t", js_expr(blk)),
                    JsStmt::synth(JsStmtNode::ForNum {
                        binding: "__i".into(),
                        limit: js_expr(recv.unwrap()),
                        body: vec![JsStmt::expr(Js::call(
                            Span::synthetic(),
                            synth_ident("__t"),
                            vec![synth_ident("__i")],
                        ))],
                    }),
                ];
                return iife(span, stmts);
            }
        }
    }
    // Async HOF rewrite (Phase 3): if the receiver is an array
    // (or array-like) AND the method is a known higher-order
    // operation AND the block body contains async sends, rewrite
    // to a `for...of` IIFE so awaits land in the iteration order
    // they'd run in Ruby. Without this, `.map(async x => ...)`
    // produces `Promise<R>[]` (parallel-pending) instead of `R[]`,
    // and `.filter(async x => ...)` doesn't await predicates at
    // all (the JS array methods don't introspect their callbacks).
    if let Some(rewritten) = try_js_async_hof(span, recv, method, args, blk) {
        return rewritten;
    }
    // Sync HOF rewrite for predicate-style Ruby Array methods
    // (`all?`/`any?`/`none?`) — the `?` sanitizer renames them to
    // `is_all`/`is_any`/`is_none` which don't exist on JS Array.
    // Restricted to predicates because the type-aware Array dispatch
    // (Ty::Array) handles `each`/`map`/`filter`/`find` for typed
    // receivers; broadening this fallback to those methods misfires
    // when the receiver is actually a Hash.
    if args.is_empty() && recv.is_some() {
        if let ExprNode::Lambda { body, .. } = &*blk.node {
            if !body_has_async_send(body) {
                let recv_js = || js_expr(recv.unwrap());
                match method {
                    "all?" => {
                        return Js::method_call(span, recv_js(), "every", vec![js_expr(blk)])
                    }
                    "any?" => return Js::method_call(span, recv_js(), "some", vec![js_expr(blk)]),
                    "none?" => {
                        return Js::unary(
                            span,
                            "!",
                            Js::method_call(span, recv_js(), "some", vec![js_expr(blk)]),
                        )
                    }
                    _ => {}
                }
            }
        }
    }
    let mut all_args: Vec<Expr> = args.to_vec();
    all_args.push(blk.clone());
    js_send(span, recv, method, &all_args, true)
}

/// HOF rewrites for blocks containing async sends. Returns
/// `Some(rewrite)` when the (method, block) shape matches a known
/// HOF and the block body has at least one async Send; otherwise
/// `None` (caller falls back to the standard chained-call emit).
///
/// Each rewrite produces an `await (async () => { ... })()`
/// expression so the IIFE's Promise return is awaited at the call
/// site — the surrounding async-coloring machinery doesn't know
/// `each`/`map`/etc are now async (they aren't in the extern set),
/// so the rewrite must own the outer `await`.
fn try_js_async_hof(
    span: Span,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: &Expr,
) -> Option<Js> {
    // Block must be a Lambda — that's the only shape carrying
    // params + body in a way we can splice into a for-of header.
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return None;
    };
    if !body_has_async_send(body) {
        return None;
    }
    let recv = recv?;

    // Single-param HOFs: the bound name from the block's `params[0]`
    // becomes the for-of binding; the body emits with that name in
    // scope (the body-typer already assigned the right VarId at
    // ingest, so js_expr will pick it up).
    let single_param = || -> Option<String> { Some(params.first()?.as_str().to_string()) };
    let for_of = |binding: String, body_stmts: Vec<JsStmt>| {
        JsStmt::synth(JsStmtNode::ForOf { binding, iterable: js_expr(recv), body: body_stmts })
    };

    match method {
        "each" if args.is_empty() => {
            let p = single_param()?;
            Some(async_iife_awaited(span, vec![for_of(p, expr_stmts(body))]))
        }
        "map" | "collect" if args.is_empty() => {
            let p = single_param()?;
            let push = Js::method_call(
                Span::synthetic(),
                synth_ident("__r"),
                "push",
                vec![js_expr(body)],
            );
            Some(async_iife_awaited(
                span,
                vec![
                    const_decl("__r", Js::synth(JsExpr::Array(vec![]))),
                    for_of(p, vec![JsStmt::expr(push)]),
                    return_stmt(Some(synth_ident("__r"))),
                ],
            ))
        }
        "filter" | "select" if args.is_empty() => {
            let p = single_param()?;
            let push = Js::method_call(
                Span::synthetic(),
                synth_ident("__r"),
                "push",
                vec![synth_ident(&p)],
            );
            let guard = JsStmt::synth(JsStmtNode::If {
                cond: js_expr(body),
                then: vec![JsStmt::expr(push)],
                else_: None,
            });
            Some(async_iife_awaited(
                span,
                vec![
                    const_decl("__r", Js::synth(JsExpr::Array(vec![]))),
                    for_of(p, vec![guard]),
                    return_stmt(Some(synth_ident("__r"))),
                ],
            ))
        }
        "reject" if args.is_empty() => {
            let p = single_param()?;
            let push = Js::method_call(
                Span::synthetic(),
                synth_ident("__r"),
                "push",
                vec![synth_ident(&p)],
            );
            let guard = JsStmt::synth(JsStmtNode::If {
                cond: Js::unary(Span::synthetic(), "!", js_expr(body)),
                then: vec![JsStmt::expr(push)],
                else_: None,
            });
            Some(async_iife_awaited(
                span,
                vec![
                    const_decl("__r", Js::synth(JsExpr::Array(vec![]))),
                    for_of(p, vec![guard]),
                    return_stmt(Some(synth_ident("__r"))),
                ],
            ))
        }
        "find" | "detect" if args.is_empty() => {
            let p = single_param()?;
            let found = JsStmt::synth(JsStmtNode::If {
                cond: js_expr(body),
                then: vec![return_stmt(Some(synth_ident(&p)))],
                else_: None,
            });
            Some(async_iife_awaited(
                span,
                vec![for_of(p, vec![found]), return_stmt(Some(synth_ident("undefined")))],
            ))
        }
        "any?" if args.is_empty() => {
            let p = single_param()?;
            let hit = JsStmt::synth(JsStmtNode::If {
                cond: js_expr(body),
                then: vec![return_stmt(Some(Js::synth(JsExpr::Bool(true))))],
                else_: None,
            });
            Some(async_iife_awaited(
                span,
                vec![for_of(p, vec![hit]), return_stmt(Some(Js::synth(JsExpr::Bool(false))))],
            ))
        }
        "all?" if args.is_empty() => {
            let p = single_param()?;
            let miss = JsStmt::synth(JsStmtNode::If {
                cond: Js::unary(Span::synthetic(), "!", js_expr(body)),
                then: vec![return_stmt(Some(Js::synth(JsExpr::Bool(false))))],
                else_: None,
            });
            Some(async_iife_awaited(
                span,
                vec![for_of(p, vec![miss]), return_stmt(Some(Js::synth(JsExpr::Bool(true))))],
            ))
        }
        "reduce" | "inject" if args.len() == 1 && params.len() == 2 => {
            // `arr.reduce(init) { |acc, x| op }` — accumulator
            // pattern. `params[0]` is the accumulator name,
            // `params[1]` is the element.
            let acc = params[0].as_str().to_string();
            let elem = params[1].as_str().to_string();
            let acc_decl = JsStmt::synth(JsStmtNode::VarDecl {
                kind: VarKind::Let,
                name: acc.clone(),
                ty: None,
                init: Some(js_expr(&args[0])),
            });
            let step = JsStmt::expr(Js::synth(JsExpr::Assign {
                target: synth_ident(&acc),
                op: "=",
                value: js_expr(body),
            }));
            Some(async_iife_awaited(
                span,
                vec![acc_decl, for_of(elem, vec![step]), return_stmt(Some(synth_ident(&acc)))],
            ))
        }
        _ => None,
    }
}

/// Core send emission, with the async-coloring await wrap. Wrapping
/// happens structurally: a `Cast` result takes the await INSIDE the
/// cast (`(await <expr>) as <Class>` — awaiting after the cast would
/// read as casting `Promise<Class>` to `Class`, which TypeScript
/// rejects with TS2352); anything else gets a plain `await` prefix
/// whose parenthesization the printer derives from context.
fn js_send(
    span: Span,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    parenthesized: bool,
) -> Js {
    let result = js_send_inner(span, recv, method, args, parenthesized);
    if !is_async_method_name(method) {
        return result;
    }
    // Receiver-aware filter: mirror the propagation-side
    // `recv_is_known_sync` check so emit doesn't wrap a Hash/Array/
    // ErrorCollection/Parameters-typed receiver with `await` for an
    // AR-adapter-named method.
    if recv_is_known_sync_at_emit(recv) {
        return result;
    }
    // Parameter-name filter: a bare `Send { recv: None, method }`
    // whose method matches an enclosing parameter name is a Var
    // read (Ruby implicit-self), not a method dispatch.
    if recv.is_none() && is_enclosing_param_name(method) {
        return result;
    }
    if let JsExpr::Cast { .. } = &*result.node {
        let result_span = result.span;
        let JsExpr::Cast { expr, ty } = *result.node else { unreachable!() };
        return Js::new(result_span, JsExpr::Cast { expr: Js::await_(result_span, expr), ty });
    }
    Js::await_(span, result)
}

/// True if `e` is the integer literal `-1`. Ruby's `i..-1` inclusive
/// range is the idiomatic "to end of sequence" form and lowers to an
/// open-ended JS slice rather than the generic `end + 1` shift (see
/// the inclusive-end branch in `js_send_inner` for the off-by-one
/// rationale). The `-1` is stored directly as a signed
/// `Literal::Int { value: -1 }` in the IR — there's no separate
/// `UnaryOp::Neg` node — so a literal-only pattern is exhaustive.
fn is_lit_neg_one(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Int { value: -1 } })
}

fn js_send_inner(
    span: Span,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    parenthesized: bool,
) -> Js {
    // Temporal reader intrinsic: `ActiveSupport.parse_db_time(s)` parses
    // stored ISO-8601 text into a native `Date` (nil-safe: `string | null`
    // → `Date | null`). Maps to the hand-written `RhDateTime.parse`
    // runtime helper (src/datetime.ts). The arg is the column's `string`
    // storage backing (`@col` → `this._col`), reached through the normal
    // ivar-read path. See the temporal branch in typescript.rs's
    // `js_library_class`.
    if method == "parse_db_time" && args.len() == 1 {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.last().map(|s| s.as_str()) == Some("ActiveSupport") {
                    return Js::method_call(
                        span,
                        Js::ident(span, "RhDateTime"),
                        "parse",
                        vec![js_expr(&args[0])],
                    );
                }
            }
        }
    }
    // Framework class-instance receivers route bracket access to
    // method dispatch (`.get(k)` / `.set(k, v)`). JS bracket access
    // on a class instance returns `undefined` for runtime keys
    // (no index signature on the class shape); the framework runtime
    // classes (Parameters, HashWithIndifferentAccess, …) expose
    // explicit `get` / `set` methods as their cross-target API.
    //
    // Hash-typed and Array-typed receivers fall through to the
    // bracket-access form below. Same hardcoded class-name list as
    // the zero-arg-method fix; goes away once the typer plumbs
    // `AccessorKind` to Send.
    let is_framework_class_recv = |r: &Expr| -> bool {
        let recv_ty = strip_nullable(r.ty.as_ref());
        matches!(
            recv_ty,
            Some(Ty::Class { id, .. }) if {
                let name = id.0.as_str();
                let last = name.rsplit("::").next().unwrap_or(name);
                matches!(last, "Parameters" | "ParameterMissing" | "Router")
            }
        )
    };
    if method == "[]" && args.len() == 1 {
        if let Some(r) = recv {
            // Ruby's `x[i..j]` slice form — when the indexer's argument
            // is a Range, lower to `.slice(i, j+1)` (or `.slice(i)` for
            // an open-ended range, or `.slice(i, j)` for an exclusive
            // range). Works for Str AND Array receivers; both have
            // `.slice` with matching JS semantics.
            if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                let begin_js = begin
                    .as_ref()
                    .map(|b| js_expr(b))
                    .unwrap_or_else(|| Js::num(Span::synthetic(), "0"));
                let recv_js = js_expr(r);
                return match end {
                    None => Js::method_call(span, recv_js, "slice", vec![begin_js]),
                    // `x[i..-1]` — inclusive range with literal `-1` end
                    // — is Ruby's "from i to last char/element". The
                    // generic `+1` shift below would produce
                    // `.slice(i, -1 + 1)` = `.slice(i, 0)` = empty
                    // (off-by-one straddling the zero boundary). Emit
                    // the open-ended form instead. The `[i..-n]` form
                    // for n ≥ 2 still takes the `+1` shift — those
                    // produce valid negative slice indices.
                    Some(end_e) if !*exclusive && is_lit_neg_one(end_e) => {
                        Js::method_call(span, recv_js, "slice", vec![begin_js])
                    }
                    Some(end_e) => {
                        let end_js = js_expr(end_e);
                        let end_js = if *exclusive {
                            end_js
                        } else {
                            Js::binary(span, "+", end_js, Js::num(Span::synthetic(), "1"))
                        };
                        Js::method_call(span, recv_js, "slice", vec![begin_js, end_js])
                    }
                };
            }
        }
    }
    if method == "[]" && args.len() == 2 {
        if let Some(r) = recv {
            // Ruby's two-arg `str[start, length]` / `arr[start, length]`
            // — substring/subarray of the given length. TS string and
            // array both expose `.slice(start, end)` with the same
            // start-inclusive/end-exclusive semantics, so the rewrite
            // is `recv.slice(start, start + length)`. Without this, the
            // generic `recv[a, b]` fallback produces JS `recv[(a, b)]`
            // (comma operator) — silently wrong.
            let end_js = Js::binary(span, "+", js_expr(&args[0]), js_expr(&args[1]));
            return Js::method_call(
                span,
                js_expr(r),
                "slice",
                vec![js_expr(&args[0]), end_js],
            );
        }
    }
    // Negative-int index on an Array (`arr[-1]` = last element,
    // `arr[-2]` = second to last, …). JS arrays don't support
    // negative indexing — `arr[-1]` reads the property "-1", which
    // is `undefined` for ordinary arrays. Rewrite to
    // `arr[arr.length - n]`. For dynamic negative indices the same
    // rewrite would need a runtime check, but the literal case covers
    // the framework patterns we ship today (`records[-1]` in
    // `Base.last`'s body).
    if method == "[]" && args.len() == 1 {
        if let (Some(r), ExprNode::Lit { value: Literal::Int { value } }) =
            (recv, &*args[0].node)
        {
            if *value < 0 && matches!(r.ty.as_ref(), Some(Ty::Array { .. }) | Some(Ty::Str)) {
                let idx = Js::binary(
                    span,
                    "-",
                    Js::member(span, js_expr(r), "length"),
                    Js::num(Span::synthetic(), (-*value).to_string()),
                );
                return Js::index(span, js_expr(r), idx);
            }
        }
    }
    if method == "[]" && args.len() == 1 {
        if let Some(r) = recv {
            if is_framework_class_recv(r) {
                return Js::method_call(span, js_expr(r), "get", vec![js_expr(&args[0])]);
            }
            // Legacy hardcode for `@params[:k]` — @params's ty isn't
            // always recovered as Class (it can flow as Hash[Sym, Any]
            // through the analyzer), so the ivar-name shortcut still
            // pays for itself. Subsumed by the framework-class match
            // above when the type is recovered.
            if matches!(&*r.node, ExprNode::Ivar { name } if name.as_str() == "params") {
                return Js::method_call(span, js_expr(r), "get", vec![js_expr(&args[0])]);
            }
        }
    }
    if method == "[]" {
        if let Some(r) = recv {
            let index = if args.len() == 1 {
                js_expr(&args[0])
            } else {
                // Multi-arg bracket beyond the handled shapes (no JS
                // analog) — joined verbatim, matching the legacy emit.
                Js::synth(JsExpr::Raw(
                    args.iter()
                        .map(|a| super::printer::render_expr(&js_expr(a)))
                        .collect::<Vec<_>>()
                        .join(", "),
                ))
            };
            return Js::index(span, js_expr(r), index);
        }
    }
    // `recv.[]=(k, v)` — indexed assignment lowered to a Send.
    if method == "[]=" && args.len() == 2 {
        if let Some(r) = recv {
            if is_framework_class_recv(r) {
                return Js::method_call(
                    span,
                    js_expr(r),
                    "set",
                    vec![js_expr(&args[0]), js_expr(&args[1])],
                );
            }
            // Default: `recv[k] = v`.
            return Js::new(
                span,
                JsExpr::Assign {
                    target: Js::index(span, js_expr(r), js_expr(&args[0])),
                    op: "=",
                    value: js_expr(&args[1]),
                },
            );
        }
    }
    // Attribute-writer Send: `obj.foo=(v)` → `obj.foo = v`. Ruby's
    // setter sugar dispatches as a method call on the `foo=` name;
    // TS uses property-assignment syntax. Only fires when the method
    // name ends in `=` (so `==` and `!=` aren't caught), excludes
    // operator names (`<=`, `>=`, etc. are binops handled elsewhere).
    if method.ends_with('=')
        && method.len() >= 2
        && method.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
        && recv.is_some()
        && args.len() == 1
    {
        let attr = &method[..method.len() - 1];
        // A temporal-column writer-send (`x.created_at = v`) targets the
        // `_<col>` string backing (the base name is the read-only `Date`
        // getter); `storage_field_name` is identity for non-temporal.
        return Js::new(
            span,
            JsExpr::Assign {
                target: Js::member(span, js_expr(recv.unwrap()), storage_field_name(attr)),
                op: "=",
                value: js_expr(&args[0]),
            },
        );
    }
    // `Target.new(args)` → `new Target(args)`. Ruby's standard
    // constructor call convention. Special cases for built-in types
    // whose JS-side construction syntax diverges:
    //   `String.new` → `""` (JS `new String()` produces a String
    //     OBJECT, not a primitive — different semantics for `+=`,
    //     equality, etc.)
    //   `Array.new` → `[]`
    //   `Hash.new` → `{}`
    if method == "new" && recv.is_some() {
        let recv_js = js_expr(recv.unwrap());
        let recv_s = super::printer::render_expr(&recv_js);
        // Heuristic: only treat `.new(...)` as a constructor call when
        // the receiver is a bare class identifier (e.g. `Article`,
        // `Comment`). Member-access receivers like `Views.Articles`
        // refer to namespaced module-of-functions where `new` is just
        // a method name (the `new` action's view function); emitting
        // `new Views.Articles(...)` would invoke an object as a
        // constructor, which TS rejects at runtime. Fall through to
        // the regular member-call form for those.
        if !recv_s.contains('.') {
            if args.is_empty() {
                match recv_s.as_str() {
                    "String" => return Js::str(span, ""),
                    "Array" => return Js::new(span, JsExpr::Array(vec![])),
                    "Hash" => return Js::new(span, JsExpr::Object(vec![])),
                    _ => {}
                }
            }
            return Js::new(
                span,
                JsExpr::New { callee: recv_js, args: args.iter().map(js_expr).collect() },
            );
        }
    }
    // `x.nil?` → `x == null` (loose equality — matches both null
    // AND undefined). Ruby's `nil?` returns true for any unset ivar
    // (Ruby reads unset @vars as nil); the TS analog of "unset"
    // is `undefined`, not `null`. Strict `=== null` would miss
    // unset class fields and break the model constructor →
    // `fill_timestamps` path. Loose `== null` is safe against the
    // false-vs-nil concern: `false == null`, `0 == null`, and
    // `"" == null` are all false in JS.
    if method == "nil?" && recv.is_some() && args.is_empty() {
        return Js::binary(span, "==", js_expr(recv.unwrap()), Js::synth(JsExpr::Null));
    }
    // `x.class` (Ruby reflection — returns the receiver's class
    // object) → `x.constructor` in TS, which exposes the same
    // surface (static methods like `table_name`, `name`). Cast
    // through `any` so downstream property access on the
    // dynamically-typed constructor doesn't trip strict mode.
    if method == "class" && recv.is_some() && args.is_empty() {
        return Js::new(
            span,
            JsExpr::Cast {
                expr: Js::member(span, js_expr(recv.unwrap()), "constructor"),
                ty: TsType("any".into()),
            },
        );
    }
    // `Time.now` → `new Date()`. Ruby's Time class has no JS
    // analog; JS Date covers the use cases the framework runtime
    // needs (`utc`, `iso8601`).
    if method == "now" && args.is_empty() {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.len() == 1 && path[0].as_str() == "Time" {
                    return Js::new(
                        span,
                        JsExpr::New { callee: Js::ident(span, "Date"), args: vec![] },
                    );
                }
            }
        }
    }
    // Ruby stdlib `JSON.generate(x)` → JS `JSON.stringify(x)`. Same
    // semantics for the framework runtime's use cases. The companion
    // `JSON.parse` is identical in both languages — passes through
    // the generic Const-recv dispatch.
    if method == "generate" && args.len() == 1 {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.len() == 1 && path[0].as_str() == "JSON" {
                    return Js::method_call(
                        span,
                        Js::ident(r.span, "JSON"),
                        "stringify",
                        vec![js_expr(&args[0])],
                    );
                }
            }
        }
    }
    // Ruby stdlib `Base64.strict_encode64(x)` → portable browser+
    // Node form: UTF-8-encode via TextEncoder, byte-stringify via
    // String.fromCharCode, base64 via btoa.
    //
    // Why not `Buffer.from(x).toString("base64")`: Buffer is a Node
    // global, undefined in browsers / SharedWorker / dedicated
    // Worker. The portable path works everywhere — Node has `btoa`,
    // `TextEncoder`, and `String.fromCharCode` as globals since 16.
    // `strict_encode64` differs from `encode64` only in not inserting
    // newlines; `btoa` is already strict-shaped, matching Ruby.
    //
    // The `String.fromCharCode(...arr)` spread has an argument-
    // count limit (~100k on most engines), but real call sites
    // here are small (turbo_stream_from channel identifiers, JSON
    // keyed by short model names). If a site ever needs unbounded
    // bytes, swap for a chunked loop in the framework runtime
    // rather than complicating this emit.
    if method == "strict_encode64" && args.len() == 1 {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.len() == 1 && path[0].as_str() == "Base64" {
                    let encoded = Js::method_call(
                        span,
                        Js::synth(JsExpr::New { callee: synth_ident("TextEncoder"), args: vec![] }),
                        "encode",
                        vec![js_expr(&args[0])],
                    );
                    let bytes = Js::method_call(
                        span,
                        synth_ident("String"),
                        "fromCharCode",
                        vec![Js::synth(JsExpr::Spread(encoded))],
                    );
                    return Js::call(span, Js::ident(span, "btoa"), vec![bytes]);
                }
            }
        }
    }
    // `<date>.utc` → no-op chained access; Date already represents
    // an absolute UTC instant. Recognize the `new Date()` form so
    // `Time.now.utc` collapses to `new Date()` cleanly; otherwise
    // fall through and keep the chain readable as-is.
    if method == "utc" && args.is_empty() && recv.is_some() {
        let inner = js_expr(recv.unwrap());
        let is_new_date = matches!(
            &*inner.node,
            JsExpr::New { callee, args }
                if args.is_empty() && matches!(&*callee.node, JsExpr::Ident(n) if n == "Date")
        );
        if is_new_date {
            return inner;
        }
    }
    // `<date>.iso8601` → `.toISOString()` — produces the Z-suffix
    // ISO-8601 string Ruby's Time#iso8601 does.
    if method == "iso8601" && args.is_empty() && recv.is_some() {
        return Js::method_call(span, js_expr(recv.unwrap()), "toISOString", vec![]);
    }
    // `<regex>.match?(s)` → `<regex>.test(s)`. Ruby's `Regexp#match?`
    // returns boolean; JS RegExp has `.test()` for the same purpose.
    // Both `match?` (predicate) and `match` (returns MatchData) get
    // mapped: predicate → `.test()`, value form → `.exec()`.
    if method == "match?" && args.len() == 1 && recv.is_some() {
        return Js::method_call(span, js_expr(recv.unwrap()), "test", vec![js_expr(&args[0])]);
    }
    if method == "match" && args.len() == 1 && recv.is_some() {
        if let Some(Ty::Class { id, .. }) = recv.unwrap().ty.as_ref() {
            if id.0.as_str() == "Regexp" || id.0.as_str() == "RegExp" {
                return Js::method_call(
                    span,
                    js_expr(recv.unwrap()),
                    "exec",
                    vec![js_expr(&args[0])],
                );
            }
        }
    }
    // Ruby coercions: `.to_s` / `.to_i` / `.to_sym` map to JS
    // equivalents. `.to_sym` is a no-op in JS (use the string as
    // the hash key) — emit just the receiver. The nil case
    // diverges from Ruby (Ruby's nil.to_s is "" but JS String(null)
    // is "null"); call sites that care should narrow first.
    if let Some(r) = recv {
        if args.is_empty() {
            match method {
                "to_s" => return Js::call(span, Js::ident(span, "String"), vec![js_expr(r)]),
                "to_i" => return Js::call(span, Js::ident(span, "Number"), vec![js_expr(r)]),
                "to_sym" => return js_expr(r),
                _ => {}
            }
        }
    }
    // `x.is_a?(ClassRef)` → JS form. Most Ruby classes are
    // user-defined and translate to `x instanceof ClassRef`, but
    // primitives in Ruby (String, Integer, Float, Numeric, Symbol)
    // are JS primitives, not class instances — `"abc" instanceof
    // String` is false in JS. Map those to their `typeof` form.
    // Array gets `Array.isArray(x)` (cross-realm safe) instead of
    // `instanceof Array`.
    if method == "is_a?" && recv.is_some() && args.len() == 1 {
        let r = recv.unwrap();
        let class_name = match &*args[0].node {
            ExprNode::Const { path } if path.len() == 1 => Some(path[0].as_str()),
            _ => None,
        };
        let typeof_is =
            |s: &str| Js::binary(span, "===", Js::unary(span, "typeof ", js_expr(r)), Js::str(span, s));
        let is_array =
            || Js::method_call(span, synth_ident("Array"), "isArray", vec![js_expr(r)]);
        return match class_name {
            Some("String") => typeof_is("string"),
            Some("Integer") => {
                Js::method_call(span, synth_ident("Number"), "isInteger", vec![js_expr(r)])
            }
            Some("Float") => Js::binary(
                span,
                "&&",
                typeof_is("number"),
                Js::unary(
                    span,
                    "!",
                    Js::method_call(span, synth_ident("Number"), "isInteger", vec![js_expr(r)]),
                ),
            ),
            Some("Numeric") => typeof_is("number"),
            // Ruby Symbol values render as TS strings (Lit::Sym
            // emits as a quoted string), so `is_a?(Symbol)` maps to
            // the same `typeof === "string"` check `is_a?(String)`
            // produces. Without the redirect, `typeof === "symbol"`
            // narrows TS's static type to `symbol`, which then
            // triggers TS2731 ("implicit Symbol-to-string coercion")
            // on subsequent template-literal interpolations.
            Some("Symbol") => typeof_is("string"),
            Some("Array") => is_array(),
            Some("TrueClass") | Some("FalseClass") => typeof_is("boolean"),
            // Ruby's `Hash` is a plain object in JS — no constructor
            // class to `instanceof` against. The plain-object check
            // is "typeof object && not null && not array".
            Some("Hash") => Js::binary(
                span,
                "&&",
                Js::binary(
                    span,
                    "&&",
                    typeof_is("object"),
                    Js::binary(span, "!==", js_expr(r), Js::synth(JsExpr::Null)),
                ),
                Js::unary(span, "!", is_array()),
            ),
            // `Regexp` is the Ruby builtin name; JS spells it
            // `RegExp` — same semantics, `instanceof` works once
            // the class name is corrected.
            Some("Regexp") => {
                Js::binary(span, "instanceof", js_expr(r), Js::ident(args[0].span, "RegExp"))
            }
            _ => Js::binary(span, "instanceof", js_expr(r), js_expr(&args[0])),
        };
    }
    // Kernel `raise` — the runtime_src self-rewrite leaves it as
    // Send-no-recv. Source surfaces:
    //   `raise X, "msg"`     → `throw new X("msg")`
    //   `raise X.new("msg")` → `throw new X("msg")` (already a Send)
    //   `raise "msg"`        → throws the string (bare-error form
    //                          hasn't been observed in the framework
    //                          runtime; add a case if it appears)
    // Ruby builtin error classes collapse to `Error` in the Const
    // emit, so `raise NotImplementedError, "..."` lands as
    // `throw new Error("...")` without a separate mapping here.
    if method == "raise" && recv.is_none() {
        match args {
            [class_e, msg_e] => {
                let err = Js::new(
                    span,
                    JsExpr::New { callee: js_expr(class_e), args: vec![js_expr(msg_e)] },
                );
                return iife(span, vec![JsStmt::synth(JsStmtNode::Throw(err))]);
            }
            [value] => {
                return iife(span, vec![JsStmt::synth(JsStmtNode::Throw(js_expr(value)))]);
            }
            _ => {}
        }
    }
    // Kernel `puts` / `print` / `p` / `pp` — map to `console.log`.
    // The rewrite pass leaves these as Send-no-recv (alongside `raise`)
    // so they don't pick up an inappropriate `this.` prefix in static
    // method bodies. Ruby's variants differ in inspect-vs-to_s
    // formatting and trailing-newline handling; `console.log` is close
    // enough for the diagnostic purpose these calls serve.
    if recv.is_none() && matches!(method, "puts" | "print" | "p" | "pp") {
        return Js::method_call(
            span,
            Js::ident(span, "console"),
            "log",
            args.iter().map(js_expr).collect(),
        );
    }
    // Kernel `require` / `require_relative` / `load` / `autoload`
    // — Ruby's late-bound module loading. TS resolves modules at
    // import time via ES module syntax (handled separately in the
    // file header), so call-site `require "base64"` has no analog
    // and drops to a no-op. Emitting `null` keeps the statement
    // well-formed; treeshake / minifier elide it.
    if recv.is_none() && matches!(method, "require" | "require_relative" | "load" | "autoload") {
        return Js::new(span, JsExpr::Null);
    }
    // `x.!` — the Send-channel form of unary `!`. Two surface forms
    // reach here, both meaning "logical not":
    //   Send { recv: Some(x), method: "!", args: [] }   — Ruby's x.!()
    //   Send { recv: None,    method: "!", args: [x] }  — view_to_library's
    //                                                     `not_x = send(None, "!", [x])`
    // The printer parenthesizes the operand when its precedence
    // demands it (`!(x === null)` etc.).
    if method == "!" {
        let inner: Option<&Expr> = match (recv, args) {
            (Some(r), []) => Some(r),
            (None, [a]) => Some(a),
            _ => None,
        };
        if let Some(inner) = inner {
            return Js::unary(span, "!", js_expr(inner));
        }
    }
    // Type-aware per-receiver dispatch. The receiver type may be
    // nullable (an ivar's flow-sensitive type is `Union<T, Nil>` since
    // a first read can observe nil before any assignment); strip the
    // nullable wrapper so dispatch fires on the inner type.
    if let Some(r) = recv {
        let recv_ty = strip_nullable(r.ty.as_ref());
        match recv_ty {
            // Ruby Array → JS Array (with native method renames where
            // they diverge: `.each` → `.forEach`, `.size` → `.length`).
            Some(Ty::Array { .. }) => match method {
                "each" => {
                    return if args.is_empty() {
                        Js::member(span, js_expr(r), "forEach")
                    } else {
                        Js::method_call(
                            span,
                            js_expr(r),
                            "forEach",
                            args.iter().map(js_expr).collect(),
                        )
                    };
                }
                "size" | "length" | "count" if args.is_empty() => {
                    return Js::member(span, js_expr(r), "length");
                }
                "empty?" if args.is_empty() => {
                    return Js::binary(
                        span,
                        "===",
                        Js::member(span, js_expr(r), "length"),
                        Js::num(span, "0"),
                    );
                }
                "any?" if args.is_empty() => {
                    return Js::binary(
                        span,
                        ">",
                        Js::member(span, js_expr(r), "length"),
                        Js::num(span, "0"),
                    );
                }
                "first" if args.is_empty() => {
                    return Js::index(span, js_expr(r), Js::num(span, "0"));
                }
                "last" if args.is_empty() => {
                    let idx = Js::binary(
                        span,
                        "-",
                        Js::member(span, js_expr(r), "length"),
                        Js::num(Span::synthetic(), "1"),
                    );
                    return Js::index(span, js_expr(r), idx);
                }
                // Ruby's `arr.reverse` returns a new array; JS Array
                // has the same name but mutates in place. Pair it with
                // a `[...arr]` spread so the receiver isn't clobbered.
                "reverse" if args.is_empty() => {
                    let copy =
                        Js::new(span, JsExpr::Array(vec![Js::synth(JsExpr::Spread(js_expr(r)))]));
                    return Js::method_call(span, copy, "reverse", vec![]);
                }
                // `arr.to_a` is a no-op on arrays; arr.to_h converts
                // a `[[k, v], ...]` array to an object.
                "to_a" if args.is_empty() => return js_expr(r),
                "to_h" if args.is_empty() => {
                    return Js::method_call(
                        span,
                        synth_ident("Object"),
                        "fromEntries",
                        vec![js_expr(r)],
                    );
                }
                // `arr.sort_by { |x| key(x) }` returns a new array
                // sorted by the key. JS Array#sort takes a comparator
                // (returning -1/0/+1) not a key function — wrap via
                // an IIFE that captures the key lambda once and
                // applies the standard ka<kb / ka>kb comparator.
                // `[...arr]` makes a copy (Ruby sort_by is
                // non-mutating; JS sort mutates in place).
                "sort_by" if args.len() == 1 => {
                    let ka = const_decl(
                        "ka",
                        Js::call(Span::synthetic(), synth_ident("__key"), vec![synth_ident("a")]),
                    );
                    let kb = const_decl(
                        "kb",
                        Js::call(Span::synthetic(), synth_ident("__key"), vec![synth_ident("b")]),
                    );
                    let cmp = Js::synth(JsExpr::Ternary {
                        cond: Js::binary(
                            Span::synthetic(),
                            "<",
                            synth_ident("ka"),
                            synth_ident("kb"),
                        ),
                        then: Js::num(Span::synthetic(), "-1"),
                        else_: Js::synth(JsExpr::Ternary {
                            cond: Js::binary(
                                Span::synthetic(),
                                ">",
                                synth_ident("ka"),
                                synth_ident("kb"),
                            ),
                            then: Js::num(Span::synthetic(), "1"),
                            else_: Js::num(Span::synthetic(), "0"),
                        }),
                    });
                    let comparator = Js::synth(JsExpr::Arrow {
                        params: vec![js_param("a"), js_param("b")],
                        body: ArrowBody::Block(vec![ka, kb, return_stmt(Some(cmp))]),
                        is_async: false,
                    });
                    let copy = Js::synth(JsExpr::Array(vec![Js::synth(JsExpr::Spread(
                        synth_ident("__arr"),
                    ))]));
                    let sorted =
                        Js::method_call(Span::synthetic(), copy, "sort", vec![comparator]);
                    let shell = Js::synth(JsExpr::Arrow {
                        params: vec![js_param("__arr"), js_param("__key")],
                        body: ArrowBody::Expr(sorted),
                        is_async: false,
                    });
                    return Js::call(span, shell, vec![js_expr(r), js_expr(&args[0])]);
                }
                // `arr.sort` (no block) → JS Array#sort with default
                // comparator on a fresh copy. JS's default sort is
                // string-coerced (matches Ruby's sort for strings,
                // diverges for numbers but those need an explicit
                // comparator anyway).
                "sort" if args.is_empty() => {
                    let copy =
                        Js::new(span, JsExpr::Array(vec![Js::synth(JsExpr::Spread(js_expr(r)))]));
                    return Js::method_call(span, copy, "sort", vec![]);
                }
                // Ruby's `Array#join` with no args uses `$,` as the
                // separator (defaults to nil → ""). JS's
                // `Array.prototype.join()` defaults to "," — wrong
                // semantics. Always pass an explicit separator.
                "join" if args.is_empty() => {
                    return Js::method_call(
                        span,
                        js_expr(r),
                        "join",
                        vec![Js::str(Span::synthetic(), "")],
                    );
                }
                "join" if args.len() == 1 => {
                    return Js::method_call(span, js_expr(r), "join", vec![js_expr(&args[0])]);
                }
                _ => {}
            },
            // String receiver: predicate forms parallel Array's; the
            // case-shift helpers map to JS String methods.
            Some(Ty::Str) => match method {
                "empty?" if args.is_empty() => {
                    return Js::binary(
                        span,
                        "===",
                        Js::member(span, js_expr(r), "length"),
                        Js::num(span, "0"),
                    );
                }
                "size" | "length" if args.is_empty() => {
                    return Js::member(span, js_expr(r), "length");
                }
                "upcase" if args.is_empty() => {
                    return Js::method_call(span, js_expr(r), "toUpperCase", vec![]);
                }
                "downcase" if args.is_empty() => {
                    return Js::method_call(span, js_expr(r), "toLowerCase", vec![]);
                }
                "capitalize" if args.is_empty() => {
                    // JS has no built-in capitalize. Match Ruby's
                    // semantics: uppercase the first char, lowercase
                    // the rest. Wrap in IIFE so the receiver expr is
                    // evaluated once even though we reference it twice.
                    let s = || synth_ident("__s");
                    let head = Js::method_call(
                        Span::synthetic(),
                        Js::method_call(
                            Span::synthetic(),
                            s(),
                            "charAt",
                            vec![Js::num(Span::synthetic(), "0")],
                        ),
                        "toUpperCase",
                        vec![],
                    );
                    let tail = Js::method_call(
                        Span::synthetic(),
                        Js::method_call(
                            Span::synthetic(),
                            s(),
                            "slice",
                            vec![Js::num(Span::synthetic(), "1")],
                        ),
                        "toLowerCase",
                        vec![],
                    );
                    let shell = Js::synth(JsExpr::Arrow {
                        params: vec![js_param("__s")],
                        body: ArrowBody::Expr(Js::binary(Span::synthetic(), "+", head, tail)),
                        is_async: false,
                    });
                    return Js::call(span, shell, vec![js_expr(r)]);
                }
                "strip" if args.is_empty() => {
                    return Js::method_call(span, js_expr(r), "trim", vec![]);
                }
                "reverse" if args.is_empty() => {
                    let chars = Js::method_call(
                        span,
                        js_expr(r),
                        "split",
                        vec![Js::str(Span::synthetic(), "")],
                    );
                    let reversed = Js::method_call(span, chars, "reverse", vec![]);
                    return Js::method_call(
                        span,
                        reversed,
                        "join",
                        vec![Js::str(Span::synthetic(), "")],
                    );
                }
                "chars" if args.is_empty() => {
                    return Js::method_call(
                        span,
                        js_expr(r),
                        "split",
                        vec![Js::str(Span::synthetic(), "")],
                    );
                }
                "start_with?" if args.len() == 1 => {
                    return Js::method_call(
                        span,
                        js_expr(r),
                        "startsWith",
                        vec![js_expr(&args[0])],
                    );
                }
                "end_with?" if args.len() == 1 => {
                    return Js::method_call(
                        span,
                        js_expr(r),
                        "endsWith",
                        vec![js_expr(&args[0])],
                    );
                }
                "include?" if args.len() == 1 => {
                    return Js::method_call(
                        span,
                        js_expr(r),
                        "includes",
                        vec![js_expr(&args[0])],
                    );
                }
                // `s.sub(pat, repl)` → `s.replace(pat, repl)` (first
                // match only — JS replace's default semantics match
                // Ruby sub).
                "sub" if args.len() == 2 => {
                    return Js::method_call(
                        span,
                        js_expr(r),
                        "replace",
                        vec![js_expr(&args[0]), js_expr(&args[1])],
                    );
                }
                // `s.gsub(pat, repl)` → `s.replace(pat_with_g, repl)`.
                // Ruby gsub replaces every match; JS replace defaults
                // to first only — the regex needs a `g` flag. Patch
                // it inline when the pattern is a regex literal;
                // otherwise wrap with a runtime g-flag enforcer.
                // Hash replacements (`s.gsub(re, MAP)`) wrap in a
                // lookup callback `m => MAP[m]`.
                "gsub" if args.len() == 2 => {
                    // HTML escaper fast path: `s.gsub(HTML_ESCAPE_PATTERN,
                    // HTML_ESCAPES)`. The generic Const-pattern branch
                    // below can't see the named constant's type, so it
                    // builds `new RegExp(pat.source, pat.flags + "g")`
                    // from the source string on *every call* —
                    // recompiling the regex per request. Emit an inline
                    // literal `/[&<>"']/g` instead: a regex literal is
                    // compiled and cached once by the engine. Keyed on
                    // the `HTML_ESCAPES` constant so json_builder's
                    // escaper (different set) keeps the generic form.
                    if let ExprNode::Const { path } = &*args[1].node {
                        if path.last().map(|s| s.as_str()) == Some("HTML_ESCAPES") {
                            let pat = Js::synth(JsExpr::Regex {
                                pattern: "[&<>\"']".into(),
                                flags: "g".into(),
                            });
                            let lookup = Js::synth(JsExpr::Arrow {
                                params: vec![JsParam {
                                    name: "__m".into(),
                                    optional: false,
                                    ty: Some(TsType("string".into())),
                                    default: None,
                                }],
                                body: ArrowBody::Expr(Js::synth(JsExpr::Index {
                                    obj: js_expr(&args[1]),
                                    index: synth_ident("__m"),
                                })),
                                is_async: false,
                            });
                            return Js::method_call(
                                span,
                                js_expr(r),
                                "replace",
                                vec![pat, lookup],
                            );
                        }
                    }
                    let pat_js = if let ExprNode::Lit {
                        value: Literal::Regex { pattern, flags },
                    } = &*args[0].node
                    {
                        let new_flags = if flags.contains('g') {
                            flags.clone()
                        } else {
                            format!("{flags}g")
                        };
                        Js::new(
                            args[0].span,
                            JsExpr::Regex {
                                pattern: translate_ruby_regex_anchors(pattern),
                                flags: new_flags,
                            },
                        )
                    } else {
                        // Runtime check — covers Const refs to regex
                        // constants (`HTML_ESCAPE_PATTERN`) whose type
                        // isn't visible at emit time.
                        let raw = || js_expr(&args[0]);
                        let needs_flag = Js::binary(
                            Span::synthetic(),
                            "&&",
                            Js::binary(
                                Span::synthetic(),
                                "instanceof",
                                raw(),
                                synth_ident("RegExp"),
                            ),
                            Js::unary(
                                Span::synthetic(),
                                "!",
                                Js::method_call(
                                    Span::synthetic(),
                                    Js::member(Span::synthetic(), raw(), "flags"),
                                    "includes",
                                    vec![Js::str(Span::synthetic(), "g")],
                                ),
                            ),
                        );
                        let with_flag = Js::synth(JsExpr::New {
                            callee: synth_ident("RegExp"),
                            args: vec![
                                Js::member(Span::synthetic(), raw(), "source"),
                                Js::binary(
                                    Span::synthetic(),
                                    "+",
                                    Js::member(Span::synthetic(), raw(), "flags"),
                                    Js::str(Span::synthetic(), "g"),
                                ),
                            ],
                        });
                        Js::synth(JsExpr::Ternary {
                            cond: needs_flag,
                            then: with_flag,
                            else_: raw(),
                        })
                    };
                    let repl_js = if matches!(args[1].ty.as_ref(), Some(Ty::Hash { .. })) {
                        Js::synth(JsExpr::Arrow {
                            params: vec![JsParam {
                                name: "__m".into(),
                                optional: false,
                                ty: Some(TsType("string".into())),
                                default: None,
                            }],
                            body: ArrowBody::Expr(Js::synth(JsExpr::Index {
                                obj: js_expr(&args[1]),
                                index: synth_ident("__m"),
                            })),
                            is_async: false,
                        })
                    } else {
                        js_expr(&args[1])
                    };
                    return Js::method_call(span, js_expr(r), "replace", vec![pat_js, repl_js]);
                }
                // `s.tr(from, to)` — character translation. Limited
                // to single-char from/to (covers framework Ruby's
                // `inner_k.to_s.tr("_", "-")` shape). Multi-char
                // and ranges aren't yet supported; the call falls
                // through to the generic dispatch (and tsc errors)
                // so the gap surfaces instead of silently miscompiling.
                "tr" if args.len() == 2 => {
                    if let (
                        ExprNode::Lit { value: Literal::Str { value: from } },
                        ExprNode::Lit { value: Literal::Str { value: to } },
                    ) = (&*args[0].node, &*args[1].node)
                    {
                        if from.chars().count() == 1 && to.chars().count() == 1 {
                            // Escape the `from` char for use inside a
                            // regex character class.
                            let c = from.chars().next().unwrap();
                            let escaped = match c {
                                '\\' | '/' | '^' | '$' | '.' | '|' | '?' | '*' | '+' | '('
                                | ')' | '[' | ']' | '{' | '}' => format!("\\{c}"),
                                _ => c.to_string(),
                            };
                            let pat =
                                Js::synth(JsExpr::Regex { pattern: escaped, flags: "g".into() });
                            return Js::method_call(
                                span,
                                js_expr(r),
                                "replace",
                                vec![pat, Js::str(args[1].span, to.clone())],
                            );
                        }
                    }
                }
                _ => {}
            },
            // Hash → JS plain-object. `.merge` becomes object spread;
            // `.key?` becomes the `in` operator; `.empty?` counts keys;
            // `.each |k, v|` iterates entries.
            Some(Ty::Hash { .. }) => {
                let keys = || {
                    Js::method_call(span, synth_ident("Object"), "keys", vec![js_expr(r)])
                };
                match method {
                    "key?" | "has_key?" | "include?" if args.len() == 1 => {
                        return Js::binary(span, "in", js_expr(&args[0]), js_expr(r));
                    }
                    "empty?" if args.is_empty() => {
                        return Js::binary(
                            span,
                            "===",
                            Js::member(span, keys(), "length"),
                            Js::num(span, "0"),
                        );
                    }
                    "any?" if args.is_empty() => {
                        return Js::binary(
                            span,
                            ">",
                            Js::member(span, keys(), "length"),
                            Js::num(span, "0"),
                        );
                    }
                    "size" | "length" if args.is_empty() => {
                        return Js::member(span, keys(), "length");
                    }
                    "merge" if args.len() == 1 => {
                        return Js::new(
                            span,
                            JsExpr::Object(vec![
                                JsObjEntry::Spread(js_expr(r)),
                                JsObjEntry::Spread(js_expr(&args[0])),
                            ]),
                        );
                    }
                    // `hash.delete(key)` — Ruby removes the key in
                    // place and returns the deleted value (or nil).
                    // JS plain objects don't have `.delete()`; the
                    // `delete` keyword is the statement form. Emit
                    // an IIFE so the Send expression is valid in
                    // both expression and statement position, and so
                    // the return value matches Ruby (the prior value
                    // at the key, or `undefined` if absent — close
                    // enough to nil for the framework's call sites).
                    "delete" if args.len() == 1 => {
                        let h = || synth_ident("__h");
                        let k = || synth_ident("__k");
                        let entry = || {
                            Js::synth(JsExpr::Index { obj: h(), index: k() })
                        };
                        let stmts = vec![
                            const_decl("__v", entry()),
                            JsStmt::expr(Js::unary(Span::synthetic(), "delete ", entry())),
                            return_stmt(Some(synth_ident("__v"))),
                        ];
                        let shell = Js::synth(JsExpr::Arrow {
                            params: vec![js_param("__h"), js_param("__k")],
                            body: ArrowBody::Block(stmts),
                            is_async: false,
                        });
                        return Js::call(span, shell, vec![js_expr(r), js_expr(&args[0])]);
                    }
                    "keys" if args.is_empty() => return keys(),
                    "values" if args.is_empty() => {
                        return Js::method_call(
                            span,
                            synth_ident("Object"),
                            "values",
                            vec![js_expr(r)],
                        );
                    }
                    // `.to_h` on a Hash is a no-op in Ruby — emit the
                    // receiver verbatim. The strong-params chain
                    // (`params.require(:k).permit(:a, :b).to_h`) is
                    // the common producer.
                    "to_h" if args.is_empty() => return js_expr(r),
                    // `hash.fetch(key, default)` → `hash[key] ?? default`.
                    // Spec lowering's `<Resource>Params.from_raw` body
                    // emits `params.fetch("title", "")` for each
                    // permitted field; without this rewrite the Send
                    // emits literally and tsc rejects since
                    // `Record<string, any>` has no `.fetch`. The
                    // single-arg form becomes a bracket index — Ruby's
                    // KeyError on missing key isn't modeled in TS.
                    "fetch" if args.len() == 2 => {
                        return Js::binary(
                            span,
                            "??",
                            Js::index(span, js_expr(r), js_expr(&args[0])),
                            js_expr(&args[1]),
                        );
                    }
                    "fetch" if args.len() == 1 => {
                        return Js::index(span, js_expr(r), js_expr(&args[0]));
                    }
                    "dup" | "clone" if args.is_empty() => {
                        return Js::new(
                            span,
                            JsExpr::Object(vec![JsObjEntry::Spread(js_expr(r))]),
                        );
                    }
                    "each" if args.len() <= 1 => {
                        // `hash.each |k, v| { ... }` lowers to a
                        // 2-arg block. JS's `Object.entries(o).forEach`
                        // passes a single `[k, v]` tuple; wrap the
                        // block in a forwarder that pulls the pair
                        // apart so the caller-supplied 2-arg lambda
                        // sees `(k, v)` as Ruby intended.
                        let entries = Js::method_call(
                            span,
                            synth_ident("Object"),
                            "entries",
                            vec![js_expr(r)],
                        );
                        return if args.is_empty() {
                            entries
                        } else {
                            let pair_index = |i: &str| {
                                Js::synth(JsExpr::Index {
                                    obj: synth_ident("__p"),
                                    index: Js::num(Span::synthetic(), i),
                                })
                            };
                            let forward = Js::synth(JsExpr::Arrow {
                                params: vec![js_param("__p")],
                                body: ArrowBody::Expr(Js::call(
                                    Span::synthetic(),
                                    js_expr(&args[0]),
                                    vec![pair_index("0"), pair_index("1")],
                                )),
                                is_async: false,
                            });
                            Js::method_call(span, entries, "forEach", vec![forward])
                        };
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Ruby's `<<` is polymorphic: Int bit-shift, Array/String append,
    // or a method call on classes that define it (like
    // ActiveModel::Errors.add). Dispatch on receiver type. TS has no
    // `<<` operator overloading, so the Class case has to emit as a
    // method call; the method name is `add` by convention (matches
    // Juntos's ActiveModel::Errors and similar collection APIs).
    if method == "<<" && recv.is_some() && args.len() == 1 {
        let r = recv.unwrap();
        if let Some(recv_ty) = &r.ty {
            match recv_ty {
                Ty::Class { .. } => {
                    return Js::method_call(span, js_expr(r), "add", vec![js_expr(&args[0])]);
                }
                Ty::Array { .. } => {
                    return Js::method_call(span, js_expr(r), "push", vec![js_expr(&args[0])]);
                }
                // Ruby's `str << x` appends in place. TS strings
                // are immutable, but the receiver is always a
                // local variable in our view-builder pattern (the
                // synthesized `io` buffer), so `+=` produces the
                // same effect at the call site.
                Ty::Str => {
                    return Js::new(
                        span,
                        JsExpr::Assign { target: js_expr(r), op: "+=", value: js_expr(&args[0]) },
                    );
                }
                _ => {}
            }
        }
    }
    // Ruby's binary operators ride the Send channel. TS needs infix;
    // `==` and `!=` map to strict `===` / `!==` so equality semantics
    // match Ruby (Ruby has no implicit type coercion).
    if let (Some(r), [arg]) = (recv, args) {
        // `+` dispatch: TS's native `+` handles numeric and string;
        // Array concat wants spread. Incompatible pairs refuse.
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            match classify_add(r, arg) {
                AddCase::ArrayConcat { .. } => {
                    return Js::new(
                        span,
                        JsExpr::Array(vec![
                            Js::synth(JsExpr::Spread(js_expr(r))),
                            Js::synth(JsExpr::Spread(js_expr(arg))),
                        ]),
                    );
                }
                AddCase::Incompatible => {
                    return iife_throw_msg(span, "roundhouse: + with incompatible operand types");
                }
                _ => {}
            }
        }
        // `-` dispatch: TS's native `-` handles numerics. Array set-
        // difference uses filter + includes. Incompatible pairs refuse.
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            match classify_sub(r, arg) {
                SubCase::ArrayDifference { .. } => {
                    let pred = Js::synth(JsExpr::Arrow {
                        params: vec![js_param("x")],
                        body: ArrowBody::Expr(Js::unary(
                            Span::synthetic(),
                            "!",
                            Js::method_call(
                                Span::synthetic(),
                                js_expr(arg),
                                "includes",
                                vec![synth_ident("x")],
                            ),
                        )),
                        is_async: false,
                    });
                    return Js::method_call(span, js_expr(r), "filter", vec![pred]);
                }
                SubCase::Incompatible => {
                    return iife_throw_msg(span, "roundhouse: - with incompatible operand types");
                }
                _ => {}
            }
        }
        // `*` dispatch: TS's native `*` handles numerics. String repeat
        // uses `.repeat(n)`; array repeat has no built-in (fill+flat
        // trick); array join uses `.join(sep)`.
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            match classify_mul(r, arg) {
                MulCase::StringRepeat => {
                    return Js::method_call(span, js_expr(r), "repeat", vec![js_expr(arg)]);
                }
                MulCase::ArrayRepeat { .. } => {
                    // Array(n).fill(lhs).flat() repeats the array n times.
                    let filled = Js::method_call(
                        span,
                        Js::call(span, Js::ident(span, "Array"), vec![js_expr(arg)]),
                        "fill",
                        vec![js_expr(r)],
                    );
                    return Js::method_call(span, filled, "flat", vec![]);
                }
                MulCase::ArrayJoin { .. } => {
                    return Js::method_call(span, js_expr(r), "join", vec![js_expr(arg)]);
                }
                MulCase::Incompatible => {
                    return iife_throw_msg(span, "roundhouse: * with incompatible operand types");
                }
                _ => {}
            }
        }
        // `/` and `**` dispatch: TS has both as native operators. Only
        // Incompatible pairs need special handling.
        if method == "/" || method == "**" {
            use crate::emit::shared::div_pow::{classify_div_pow, DivPowCase};
            if matches!(classify_div_pow(r, arg), DivPowCase::Incompatible) {
                return iife_throw_msg(
                    span,
                    &format!("roundhouse: `{method}` with incompatible operand types"),
                );
            }
        }
        // `%` dispatch: TS has native `%` for numerics; Str % args
        // (Ruby sprintf) has no JS/TS equivalent — emit a throw.
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            match classify_modulo(r, arg) {
                ModuloCase::StringFormat => {
                    return iife_throw_msg(
                        span,
                        "roundhouse: String % (sprintf) not yet supported for TypeScript target",
                    );
                }
                ModuloCase::Incompatible => {
                    return iife_throw_msg(span, "roundhouse: % with incompatible operand types");
                }
                _ => {}
            }
        }
        // Comparison dispatch: between two Class refs, Ruby's `<`/`<=`/
        // `>`/`>=` are Module#<-family subclass-relation checks, not
        // value comparison. JS has no native equivalent; lower to a
        // prototype-chain check via `instanceof` on the prototype.
        // (`A.prototype instanceof B` is true iff A is a strict
        // subclass of B; strict subclass + identity covers <=/>=.)
        if matches!(method, "<" | "<=" | ">" | ">=") {
            use crate::emit::shared::cmp::{classify_cmp, CmpCase};
            if matches!(classify_cmp(r, arg), CmpCase::ClassSubclass) {
                let strict = |sub: &Expr, sup: &Expr| {
                    Js::binary(
                        span,
                        "instanceof",
                        Js::member(Span::synthetic(), js_expr(sub), "prototype"),
                        js_expr(sup),
                    )
                };
                let identity = || Js::binary(span, "===", js_expr(r), js_expr(arg));
                return match method {
                    "<" => strict(r, arg),
                    "<=" => Js::binary(span, "||", identity(), strict(r, arg)),
                    ">" => strict(arg, r),
                    ">=" => Js::binary(span, "||", identity(), strict(arg, r)),
                    _ => unreachable!(),
                };
            }
        }
        if let Some(op) = ts_binop(method) {
            return Js::binary(span, op, js_expr(r), js_expr(arg));
        }
    }
    // Ruby stdlib method → TS equivalent, when the Ruby name collides
    // with a nonexistent TS property. Keyed on name only today; a
    // receiver-typed dispatch would replace this when per-type
    // mappings diverge.
    //
    // `include?` (no type info) → `.includes(...)` — the Array dispatch
    // covers known-Array receivers above; this catches the case
    // where receiver type is `any`. Hash receivers reach the
    // type-aware branch and emit as `in`, so they don't fall through
    // here.
    let (mapped_name, force_parens) = match method {
        "strip" => ("trim", true),
        "include?" => ("includes", true),
        _ => (method, false),
    };
    let ts_m = ts_method_name(mapped_name);
    match recv {
        None => {
            if args.is_empty() {
                Js::ident(span, ts_m)
            } else {
                Js::call(span, Js::ident(span, ts_m), args.iter().map(js_expr).collect())
            }
        }
        Some(r) => {
            // Ruby's `obj.name` without parens is typically a reader;
            // Juntos mirrors that with a property accessor / getter,
            // so emit without parens for instance receivers.
            //
            // EXCEPTION: when the receiver is a `Const` (a namespace
            // import like `ViewHelpers`, `RouteHelpers`, `Inflector`,
            // `Array`, `String`, `Math`, …), zero-arg sends are
            // function CALLS, not property reads — those namespaces
            // expose callable functions, not getters.
            //
            // SUB-EXCEPTION: a small set of class-level attr_accessor
            // fields in the framework runtime (`ActiveRecord.adapter`,
            // …) emit as `static x: T;` not as a method, so callers
            // need property access not a call. Carry the list here
            // until the typer surfaces AccessorKind through Send.
            let is_const_recv = matches!(&*r.node, ExprNode::Const { .. });
            let const_field = is_const_recv && {
                let path = if let ExprNode::Const { path } = &*r.node {
                    path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
                } else {
                    String::new()
                };
                matches!((path.as_str(), method), ("ActiveRecord", "adapter"))
            };
            let suppress_const_parens = is_const_recv && const_field;
            // Class-instance receivers whose class is one of the
            // transpiled framework runtime classes ALWAYS need `()`
            // for zero-arg method calls — these classes use explicit
            // `def` methods (Parameters, HashWithIndifferentAccess,
            // ActionDispatch::Router, etc.), not attr_reader-collapsed
            // TS fields. Without `()`, the emit produces a method
            // reference instead of a call, which JS lets stand at
            // parse time and produces wrong values at runtime.
            //
            // App-level model classes (Article, Comment, …) have their
            // attr_readers collapsed to TS fields, so zero-arg
            // `article.title` SHOULD stay as property access — those
            // aren't on this list. The body-typer carries
            // `AccessorKind` per method but doesn't yet thread it
            // through to the Send-emit; this hardcoded class-name list
            // is the bridge until that plumbing lands.
            let recv_ty_inner = strip_nullable(r.ty.as_ref());
            let is_method_class_recv = matches!(
                recv_ty_inner,
                Some(Ty::Class { id, .. }) if {
                    let name = id.0.as_str();
                    let last = name.rsplit("::").next().unwrap_or(name);
                    matches!(last, "Parameters" | "ParameterMissing" | "Router")
                }
            );
            let raw = if args.is_empty()
                && !parenthesized
                && !force_parens
                && !is_method_class_recv
                && (!is_const_recv || suppress_const_parens)
            {
                Js::member(span, js_expr(r), ts_m)
            } else {
                Js::method_call(span, js_expr(r), ts_m, args.iter().map(js_expr).collect())
            };
            // Self-type narrowing: framework Base methods like
            // `find`/`all`/`where`/`last`/`create` declare a return
            // type of `Base` in RBS, but at the call site
            // `Article.find(id)` should yield `Article`. Roundhouse's
            // RBS parser doesn't support `instance` / `self` types,
            // so emit a TS cast here when the receiver is a Const
            // class and the method is one of the self-typed Base
            // class methods. Resolves TS2740 ("Base missing
            // properties from Article") on every model assignment
            // from a class-method result. Singular methods cast to
            // `<Class>`; collection-returning methods cast to
            // `<Class>[]`.
            if is_const_recv {
                // Method names match against the Ruby form (with the
                // `!`/`?` suffix the source uses) since the IR
                // preserves them — sanitize happens later, in
                // `ts_method_name`. So `Article.create!(...)` lands
                // here with `method == "create!"`.
                let stripped = method.trim_end_matches('!').trim_end_matches('?');
                let recv_s = super::printer::render_expr(&js_expr(r));
                let cast_target = match stripped {
                    "find" | "find_by" | "last" | "create" | "first" => Some(recv_s),
                    "all" | "where" => Some(format!("{recv_s}[]")),
                    _ => None,
                };
                if let Some(target) = cast_target {
                    return Js::new(span, JsExpr::Cast { expr: raw, ty: TsType(target) });
                }
            }
            raw
        }
    }
}

/// Ruby `Foo::Bar` gets joined with `.` for TS access.
/// Framework-namespace paths (`ActionView::ViewHelpers`, …) collapse
/// to the last segment — TS emits each runtime class flat at its
/// file's module scope and imports the bare name. The import
/// collector mirrors this collapse, so the call site reaches the
/// imported name directly. Other paths (e.g. `Views::Articles` — a
/// real nested object) pass through joined. Single-segment Ruby
/// builtin error classes collapse to JS `Error` wherever they appear
/// as Const references — without this, `assert_operator(X, :<,
/// StandardError)` and similar idioms emit a bare `StandardError`
/// that's neither in scope nor importable.
fn const_name(path: &[Symbol]) -> String {
    let segs: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
    const FRAMEWORK_NAMESPACES: &[&str] = &[
        "ActionController",
        "ActiveRecord",
        "ActionView",
        "ActionDispatch",
        "ActiveSupport",
    ];
    if segs.len() >= 2 && FRAMEWORK_NAMESPACES.contains(&segs[0]) {
        segs.last().copied().unwrap_or("").to_string()
    } else if segs.len() == 1 {
        match segs[0] {
            "StandardError" | "RuntimeError" | "ArgumentError" | "TypeError" | "NameError"
            | "NoMethodError" | "NotImplementedError" | "KeyError" | "IndexError" => {
                "Error".to_string()
            }
            _ => segs[0].to_string(),
        }
    } else {
        segs.join(".")
    }
}

/// Object-literal key from a Ruby hash-key expression. Symbol and
/// string literals become quoted keys (matching the legacy emit);
/// integer literals stay bare; anything else renders verbatim.
fn js_obj_key(k: &Expr) -> JsKey {
    match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => JsKey::Str(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => JsKey::Str(value.clone()),
        ExprNode::Lit { value: Literal::Int { value } } => JsKey::Ident(value.to_string()),
        _ => JsKey::Ident(super::printer::render_expr(&js_expr(k))),
    }
}

pub(super) fn js_literal(span: Span, lit: &Literal) -> Js {
    match lit {
        Literal::Nil => Js::new(span, JsExpr::Null),
        Literal::Bool { value } => Js::new(span, JsExpr::Bool(*value)),
        Literal::Int { value } => Js::num(span, value.to_string()),
        Literal::Float { value } => {
            let s = value.to_string();
            Js::num(span, if s.contains('.') { s } else { format!("{s}.0") })
        }
        Literal::Str { value } => Js::str(span, value.clone()),
        // Ruby symbols map to string literals — the typed analyzer may
        // refine this into a discriminated-union enum later, but for
        // the scaffold a string is unambiguous and round-trips through
        // comparison as expected.
        Literal::Sym { value } => Js::str(span, value.as_str()),
        Literal::Regex { pattern, flags } => {
            // Ruby regex anchors don't have direct JS equivalents:
            //   `\A` / `\z` / `\Z` — Ruby string-boundary anchors,
            //                        absolute (don't match before/after \n).
            //   JS `^` / `$` — line-boundary anchors by default.
            //
            // Without translation, `/\A\d{5}\z/` emits literally and JS
            // treats `\A` / `\z` as escaped letters (matches the
            // characters A and z), silently breaking every Ruby pattern
            // that uses them. JS `^` / `$` without the `m` flag are
            // string-boundary in practice (no `m` → no per-line shift),
            // matching Ruby `\A` / `\z` for the framework's use cases
            // (validates_format_of, route param matching, etc.).
            Js::new(
                span,
                JsExpr::Regex {
                    pattern: translate_ruby_regex_anchors(pattern),
                    flags: flags.clone(),
                },
            )
        }
    }
}

/// Walk a Ruby regex source, replacing string-boundary anchors with
/// JS line-boundary anchors. Handles `\\` escapes so a literal
/// backslash followed by `A`/`z`/`Z` doesn't get clobbered.
/// Strict-end `\Z` differs from `\z` only in trailing-newline
/// handling — close enough to `$` for these targets.
fn translate_ruby_regex_anchors(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('A') => {
                    out.push('^');
                    chars.next();
                }
                Some('z') | Some('Z') => {
                    out.push('$');
                    chars.next();
                }
                Some('\\') => {
                    out.push('\\');
                    out.push('\\');
                    chars.next();
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn ts_binop(method: &str) -> Option<&'static str> {
    Some(match method {
        "==" => "===",
        "!=" => "!==",
        "<" => "<",
        "<=" => "<=",
        ">" => ">",
        ">=" => ">=",
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        "%" => "%",
        "**" => "**",
        "<<" => "<<",
        ">>" => ">>",
        "|" => "|",
        "&" => "&",
        "^" => "^",
        _ => return None,
    })
}

#[cfg(test)]
mod async_hof_tests {
    use super::*;
    use crate::expr::{BlockStyle, Expr, ExprNode};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn synth_var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from(name) },
        )
    }

    fn synth_send(recv: Option<Expr>, method: &str, args: Vec<Expr>, block: Option<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block,
                parenthesized: true,
            },
        )
    }

    fn synth_lambda(params: Vec<&str>, body: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                params: params.into_iter().map(Symbol::from).collect(),
                block_param: None,
                body,
                block_style: BlockStyle::Brace,
            },
        )
    }

    fn with_async<F: FnOnce() -> R, R>(names: &[&str], f: F) -> R {
        let set: std::collections::HashSet<Symbol> =
            names.iter().map(|s| Symbol::from(*s)).collect();
        with_async_methods(set, f)
    }

    #[test]
    fn each_with_async_block_rewrites_to_for_of() {
        // arr.each { |x| x.save } → for...of IIFE awaited.
        let body = synth_send(Some(synth_var("x")), "save", vec![], None);
        let block = synth_lambda(vec!["x"], body);
        let send = synth_send(Some(synth_var("arr")), "each", vec![], Some(block));
        let out = with_async(&["save"], || emit_expr(&send));
        assert!(
            out.starts_with("await (async () => {"),
            "expected awaited async IIFE prefix in: {out}"
        );
        assert!(out.contains("for (const x of arr)"), "got: {out}");
        assert!(out.contains("await x.save()"), "got: {out}");
    }

    #[test]
    fn map_with_async_block_pushes_into_accumulator() {
        // arr.map { |x| x.save } → IIFE that pushes into __r.
        let body = synth_send(Some(synth_var("x")), "save", vec![], None);
        let block = synth_lambda(vec!["x"], body);
        let send = synth_send(Some(synth_var("arr")), "map", vec![], Some(block));
        let out = with_async(&["save"], || emit_expr(&send));
        assert!(out.contains("const __r = []"), "got: {out}");
        assert!(out.contains("__r.push(await x.save())"), "got: {out}");
        assert!(out.contains("return __r"), "got: {out}");
    }

    #[test]
    fn filter_with_async_predicate_rewrites_correctly() {
        let body = synth_send(Some(synth_var("x")), "save", vec![], None);
        let block = synth_lambda(vec!["x"], body);
        let send = synth_send(Some(synth_var("arr")), "filter", vec![], Some(block));
        let out = with_async(&["save"], || emit_expr(&send));
        assert!(
            out.contains("if (await x.save()) { __r.push(x); }"),
            "got: {out}"
        );
    }

    #[test]
    fn reject_with_async_predicate_negates() {
        let body = synth_send(Some(synth_var("x")), "save", vec![], None);
        let block = synth_lambda(vec!["x"], body);
        let send = synth_send(Some(synth_var("arr")), "reject", vec![], Some(block));
        let out = with_async(&["save"], || emit_expr(&send));
        assert!(
            out.contains("if (!await x.save()) { __r.push(x); }"),
            "got: {out}"
        );
    }

    #[test]
    fn find_returns_first_match() {
        let body = synth_send(Some(synth_var("x")), "save", vec![], None);
        let block = synth_lambda(vec!["x"], body);
        let send = synth_send(Some(synth_var("arr")), "find", vec![], Some(block));
        let out = with_async(&["save"], || emit_expr(&send));
        assert!(out.contains("return x"), "got: {out}");
        assert!(out.contains("return undefined"), "got: {out}");
    }

    #[test]
    fn any_returns_boolean() {
        let body = synth_send(Some(synth_var("x")), "save", vec![], None);
        let block = synth_lambda(vec!["x"], body);
        let send = synth_send(Some(synth_var("arr")), "any?", vec![], Some(block));
        let out = with_async(&["save"], || emit_expr(&send));
        assert!(out.contains("return true"), "got: {out}");
        assert!(out.contains("return false"), "got: {out}");
    }

    #[test]
    fn reduce_threads_accumulator() {
        // arr.reduce(0) { |acc, x| acc + x.save }
        let init = Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: crate::expr::Literal::Int { value: 0 } },
        );
        // body: Send(method="+", recv=acc, args=[x.save])
        let acc_plus_save = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(synth_var("acc")),
                method: Symbol::from("+"),
                args: vec![synth_send(Some(synth_var("x")), "save", vec![], None)],
                block: None,
                parenthesized: false,
            },
        );
        let block = synth_lambda(vec!["acc", "x"], acc_plus_save);
        let send = synth_send(Some(synth_var("arr")), "reduce", vec![init], Some(block));
        let out = with_async(&["save"], || emit_expr(&send));
        assert!(out.contains("let acc = 0"), "got: {out}");
        assert!(out.contains("for (const x of arr)"), "got: {out}");
        assert!(out.contains("acc ="), "got: {out}");
        assert!(out.contains("return acc"), "got: {out}");
    }

    #[test]
    fn no_rewrite_when_block_has_no_async() {
        // arr.each { |x| x.foo } where foo is NOT async → emit
        // falls through to the standard chained-call path.
        let body = synth_send(Some(synth_var("x")), "foo", vec![], None);
        let block = synth_lambda(vec!["x"], body);
        let send = synth_send(Some(synth_var("arr")), "each", vec![], Some(block));
        let out = with_async(&["save"], || emit_expr(&send));
        assert!(
            !out.starts_with("await (async () => {"),
            "expected NO rewrite for sync-block each, got: {out}"
        );
    }

    #[test]
    fn sync_profile_no_rewrites() {
        // Empty async set → no rewrite even on a HOF method.
        let body = synth_send(Some(synth_var("x")), "save", vec![], None);
        let block = synth_lambda(vec!["x"], body);
        let send = synth_send(Some(synth_var("arr")), "each", vec![], Some(block));
        let out = with_async(&[], || emit_expr(&send));
        assert!(
            !out.starts_with("await (async () => {"),
            "sync profile must not rewrite, got: {out}"
        );
    }
}

#[cfg(test)]
mod range_slice_tests {
    //! Coverage for `x[Range]` slice-form emit. The interesting
    //! case is the inclusive-end branch: `x[i..-1]` (Ruby idiom for
    //! "from i to end") must lower to `x.slice(i)`, not the
    //! generic `x.slice(i, -1 + 1)` = `x.slice(i, 0)` = empty.
    //! Earlier emit hit the latter and silently zeroed any
    //! microsecond timestamps that `runtime/ruby/json_builder.rb`
    //! sliced out of a sqlite TEXT column.

    use super::*;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from(name) },
        )
    }

    fn int_lit(value: i64) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Int { value } },
        )
    }

    fn range(begin: Option<Expr>, end: Option<Expr>, exclusive: bool) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Range { begin, end, exclusive },
        )
    }

    fn slice_call(recv: Expr, range_arg: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from("[]"),
                args: vec![range_arg],
                block: None,
                parenthesized: false,
            },
        )
    }

    #[test]
    fn inclusive_range_to_neg_one_emits_open_ended_slice() {
        // `s[20..-1]` → `s.slice(20)`, not `s.slice(20, -1 + 1)`.
        let r = range(Some(int_lit(20)), Some(int_lit(-1)), false);
        let send = slice_call(var("s"), r);
        let out = emit_expr(&send);
        assert_eq!(out, "s.slice(20)");
    }

    #[test]
    fn inclusive_range_to_neg_two_keeps_plus_one_shift() {
        // `s[0..-2]` → `s.slice(0, -2 + 1)` (= `s.slice(0, -1)`,
        // JS semantics: "all but last char"). The +1 transform is
        // correct for any end ≤ -2; only -1 needed the special
        // case.
        let r = range(Some(int_lit(0)), Some(int_lit(-2)), false);
        let send = slice_call(var("s"), r);
        let out = emit_expr(&send);
        assert_eq!(out, "s.slice(0, -2 + 1)");
    }

    #[test]
    fn exclusive_range_to_neg_one_unchanged() {
        // `s[20...-1]` (exclusive) — emit drops the +1 shift
        // regardless. Stays as `s.slice(20, -1)`.
        let r = range(Some(int_lit(20)), Some(int_lit(-1)), true);
        let send = slice_call(var("s"), r);
        let out = emit_expr(&send);
        assert_eq!(out, "s.slice(20, -1)");
    }

    #[test]
    fn open_ended_range_emits_single_arg_slice() {
        // `s[20..]` → `s.slice(20)`. (Pre-existing behavior; this
        // test pins it so the new branch above doesn't perturb it.)
        let r = range(Some(int_lit(20)), None, false);
        let send = slice_call(var("s"), r);
        let out = emit_expr(&send);
        assert_eq!(out, "s.slice(20)");
    }

    #[test]
    fn inclusive_range_to_positive_end_keeps_plus_one_shift() {
        // `s[0..5]` → `s.slice(0, 5 + 1)`. Positive end stays on
        // the generic path.
        let r = range(Some(int_lit(0)), Some(int_lit(5)), false);
        let send = slice_call(var("s"), r);
        let out = emit_expr(&send);
        assert_eq!(out, "s.slice(0, 5 + 1)");
    }
}
