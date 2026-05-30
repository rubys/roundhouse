//! `while` → recursion (canonical counter loop).
//!
//! Rewrites a method whose body is a counter loop into two methods: the
//! original entry (pre-loop bindings + a call into the helper) and a
//! recursive helper that carries loop state through its parameters.
//!
//! ```text
//!   def self.match(method, path, table)        def self.match(method, path, table)
//!     mu = method.to_s.upcase                    mu = method.to_s.upcase
//!     i = 0                                       match__loop(method, path, table, mu, 0)
//!     while i < table.length          ──▶      end
//!       route = table[i]                       def self.match__loop(method, path, table, mu, i)
//!       return X if cond                         if i < length(table) do
//!       i += 1                                     route = Enum.at(table, i)
//!     end                                          if cond, do: X, else: (i = i + 1; match__loop(...))
//!     nil                                        else
//!   end                                            nil
//!                                                end
//! ```
//!
//! Carried state flows through the helper's *params*; the helper's
//! *return* is always the method's result. The loop body is converted
//! by `cps`: a `return X` becomes the value `X`; falling off the end of
//! the body becomes the recursive tail call (with the lowered counter
//! step in scope, so the call passes the advanced counter by name).
//!
//! **Scope (v1).** Only the canonical shape is handled:
//! - a `Class`-receiver method (module-singleton function),
//! - exactly one top-level `while`, body ending in a counter step
//!   (`i += 1` / `i -= 1`),
//! - no accumulator/field mutation (`a[k] = v`, `o.x = v`), no
//!   `break`/`next`/`yield`, no nested loop, and every condition
//!   variable bound before the loop (or a param).
//!
//! Anything else is left untouched (`transform_method` returns the
//! method unchanged) and degrades via the emitter's `report_unsupported`
//! catch-all (#28). Accumulator threading, instance-method loops, and
//! `break`/`next` are follow-ups.

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Param};
use crate::expr::{Expr, ExprNode, LValue, OpAssignOp};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

/// Transform one method. Returns `[entry, helper]` when the body is a
/// canonical counter loop, or `[method]` unchanged otherwise.
pub fn transform_method(m: MethodDef) -> Vec<MethodDef> {
    match try_transform(&m) {
        Some(pair) => pair,
        None => vec![m],
    }
}

fn try_transform(m: &MethodDef) -> Option<Vec<MethodDef>> {
    // v1: module-singleton functions only. Instance-method loops need
    // record-threading (mutation-threading subsystem) and are deferred.
    if m.receiver != MethodReceiver::Class {
        return None;
    }

    let stmts = seq_stmts(&m.body)?;

    // Exactly one loop in the whole method, at the top level of the body.
    if count_loops(&m.body) != 1 {
        return None;
    }
    let while_idx = stmts.iter().position(is_while)?;
    let (cond, while_body) = match &*stmts[while_idx].node {
        ExprNode::While { cond, body, until_form } if !until_form => (cond, body),
        _ => return None,
    };
    let pre = &stmts[..while_idx];
    let post = &stmts[while_idx + 1..];

    let loop_stmts = seq_stmts(while_body)?;
    let (last, leading) = loop_stmts.split_last()?;

    // The loop body must end in a counter step (`i += 1` / `i -= 1`),
    // lowered to a plain rebind so the recursive call passes the
    // advanced counter by name.
    let (counter, step_rebind) = lower_counter_step(last)?;

    // Reject shapes outside v1: accumulator/field mutation, break/next,
    // yield, or a second compound assignment in the loop body (the
    // trailing counter step `last` is already excluded — we scan only
    // `leading`).
    if leading.iter().any(has_unsupported) {
        return None;
    }

    // Carried locals = vars bound in the pre-loop statements, in order,
    // that are referenced in the condition or loop body.
    let pre_assigned = assigned_vars(pre);
    let carried: Vec<Symbol> = pre_assigned
        .into_iter()
        .filter(|name| refs_var(cond, name) || refs_var(while_body, name))
        .collect();

    // The counter must be one of the carried (pre-loop-bound) locals,
    // and every condition variable must be a param or carried — else a
    // loop-body-introduced variable would be threaded incorrectly.
    if !carried.iter().any(|c| c == &counter) {
        return None;
    }
    let param_names: Vec<Symbol> = m.params.iter().map(|p| p.name.clone()).collect();
    for v in referenced_vars(cond) {
        if !param_names.contains(&v) && !carried.contains(&v) {
            return None;
        }
    }

    // --- build the two methods ---
    let helper_name = Symbol::from(format!("{}__loop", m.name.as_str()).as_str());

    // Helper params: the method's own params, then the carried locals.
    let mut helper_params: Vec<Param> = m.params.clone();
    helper_params.extend(carried.iter().map(|n| Param::positional(n.clone())));

    // The recursive tail call passes every helper param by name; the
    // lowered counter step (in scope at the call site) advances `i`.
    let recurse_args: Vec<Expr> = helper_params.iter().map(|p| var(&p.name)).collect();
    let recurse_call = bareword_call(&helper_name, recurse_args);

    // Loop body with the trailing step lowered to a rebind.
    let mut body_stmts: Vec<Expr> = leading.to_vec();
    body_stmts.push(step_rebind);

    let post_value = value_of(post);
    let helper_body = syn(ExprNode::If {
        cond: cond.clone(),
        then_branch: cps(&body_stmts, &recurse_call),
        else_branch: post_value,
    });

    let helper = MethodDef {
        name: helper_name,
        receiver: MethodReceiver::Class,
        params: helper_params,
        block_param: None,
        body: helper_body,
        signature: None,
        effects: m.effects.clone(),
        enclosing_class: m.enclosing_class.clone(),
        kind: AccessorKind::Method,
        is_async: m.is_async,
        mutates_self: false,
    };

    // Entry: pre-loop statements (which bind the carried locals), then
    // the initial call into the helper.
    let mut entry_stmts: Vec<Expr> = pre.to_vec();
    entry_stmts.push(recurse_call);
    let mut entry = m.clone();
    entry.body = syn(ExprNode::Seq { exprs: entry_stmts });

    Some(vec![entry, helper])
}

// ---- CPS: loop body → expression, with a tail continuation ----------

/// Render a statement list as a single expression, where falling off
/// the end evaluates `k` (the continuation — here, the recursive call).
/// A `return X` yields `X` and ignores `k`.
fn cps(stmts: &[Expr], k: &Expr) -> Expr {
    let Some((first, rest)) = stmts.split_first() else {
        return k.clone();
    };
    match &*first.node {
        ExprNode::Return { value } => value.clone(),
        ExprNode::If { cond, then_branch, else_branch } => {
            // Both branches continue with the rest of the block, then k.
            let kr = cps(rest, k);
            syn(ExprNode::If {
                cond: cond.clone(),
                then_branch: cps(&branch_stmts(then_branch), &kr),
                else_branch: cps(&branch_stmts(else_branch), &kr),
            })
        }
        _ => {
            // Do `first`, then continue.
            let knext = cps(rest, k);
            syn(ExprNode::Seq { exprs: vec![first.clone(), knext] })
        }
    }
}

/// Statements of a branch: a `Seq`'s elements, `[]` for an empty/absent
/// branch, or the single expression otherwise.
fn branch_stmts(e: &Expr) -> Vec<Expr> {
    if is_empty(e) {
        return Vec::new();
    }
    match &*e.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![e.clone()],
    }
}

/// The post-loop value (evaluated when the condition is false). Empty →
/// `nil`; otherwise the statements as a value-producing block.
fn value_of(stmts: &[Expr]) -> Expr {
    match stmts {
        [] => syn(ExprNode::Lit { value: crate::expr::Literal::Nil }),
        [one] => one.clone(),
        many => syn(ExprNode::Seq { exprs: many.to_vec() }),
    }
}

// ---- shape detection ------------------------------------------------

fn is_while(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::While { .. })
}

fn seq_stmts(e: &Expr) -> Option<&Vec<Expr>> {
    match &*e.node {
        ExprNode::Seq { exprs } => Some(exprs),
        _ => None,
    }
}

fn is_empty(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: crate::expr::Literal::Nil })
        || matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
}

/// Recognize a trailing counter step (`i += 1` / `i -= 1`, or the
/// already-lowered `i = i + 1`). Returns the counter name and the step
/// as a plain rebind `Assign(i, i <op> n)`.
fn lower_counter_step(stmt: &Expr) -> Option<(Symbol, Expr)> {
    if let ExprNode::OpAssign { target: LValue::Var { name, .. }, op, value } = &*stmt.node {
        let method = match op {
            OpAssignOp::Add => "+",
            OpAssignOp::Sub => "-",
            _ => return None,
        };
        let lowered = syn(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: name.clone() },
            value: binop(var(name), method, value.clone()),
        });
        return Some((name.clone(), lowered));
    }
    // Already a plain `i = i <op> n` rebind of itself.
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*stmt.node {
        if let ExprNode::Send { recv: Some(r), method, .. } = &*value.node {
            if matches!(method.as_str(), "+" | "-") {
                if let ExprNode::Var { name: rn, .. } = &*r.node {
                    if rn == name {
                        return Some((name.clone(), stmt.clone()));
                    }
                }
            }
        }
    }
    None
}

/// True if `e` (a non-trailing loop-body statement, scanned deeply)
/// uses a construct outside v1's coverage.
fn has_unsupported(e: &Expr) -> bool {
    let mut bad = false;
    walk(e, &mut |n| match &*n.node {
        // Accumulator / field mutation can't be threaded yet.
        ExprNode::Assign { target: LValue::Index { .. } | LValue::Attr { .. }, .. }
        | ExprNode::OpAssign { target: LValue::Index { .. } | LValue::Attr { .. }, .. }
        // A compound assignment other than the (already-excluded) trailing step.
        | ExprNode::OpAssign { target: LValue::Var { .. }, .. }
        // Loop control / blocks / nested loops.
        | ExprNode::Break { .. }
        | ExprNode::Next { .. }
        | ExprNode::Yield { .. }
        | ExprNode::While { .. } => bad = true,
        _ => {}
    });
    bad
}

// ---- variable analysis ----------------------------------------------

/// Names bound by top-level `x = …` assignments, in order, deduped.
fn assigned_vars(stmts: &[Expr]) -> Vec<Symbol> {
    let mut out: Vec<Symbol> = Vec::new();
    for s in stmts {
        if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*s.node {
            if !out.contains(name) {
                out.push(name.clone());
            }
        }
    }
    out
}

fn refs_var(e: &Expr, name: &Symbol) -> bool {
    let mut found = false;
    walk(e, &mut |n| {
        if let ExprNode::Var { name: vn, .. } = &*n.node {
            if vn == name {
                found = true;
            }
        }
    });
    found
}

fn referenced_vars(e: &Expr) -> Vec<Symbol> {
    let mut out: Vec<Symbol> = Vec::new();
    walk(e, &mut |n| {
        if let ExprNode::Var { name, .. } = &*n.node {
            if !out.contains(name) {
                out.push(name.clone());
            }
        }
    });
    out
}

fn count_loops(e: &Expr) -> usize {
    let mut n = 0;
    walk(e, &mut |x| {
        if matches!(&*x.node, ExprNode::While { .. }) {
            n += 1;
        }
    });
    n
}

// ---- IR builders + a generic pre-order walker -----------------------

fn syn(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn var(name: &Symbol) -> Expr {
    syn(ExprNode::Var { id: VarId(0), name: name.clone() })
}

fn binop(lhs: Expr, method: &str, rhs: Expr) -> Expr {
    syn(ExprNode::Send {
        recv: Some(lhs),
        method: Symbol::from(method),
        args: vec![rhs],
        block: None,
        parenthesized: false,
    })
}

fn bareword_call(name: &Symbol, args: Vec<Expr>) -> Expr {
    syn(ExprNode::Send {
        recv: None,
        method: name.clone(),
        args,
        block: None,
        parenthesized: true,
    })
}

/// Pre-order walk visiting every sub-expression (best-effort over the
/// variants this pass inspects). Used for reference / shape scans.
fn walk(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    match &*e.node {
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for x in exprs {
                walk(x, f);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, f);
            }
            for a in args {
                walk(a, f);
            }
            if let Some(b) = block {
                walk(b, f);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, f);
            walk(then_branch, f);
            walk(else_branch, f);
        }
        ExprNode::While { cond, body, .. } => {
            walk(cond, f);
            walk(body, f);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, f);
            walk(right, f);
        }
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => walk(value, f),
        ExprNode::Return { value } | ExprNode::Raise { value } | ExprNode::Splat { value } => {
            walk(value, f)
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                walk(v, f);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                walk(a, f);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, f);
                walk(v, f);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    walk(expr, f);
                }
            }
        }
        ExprNode::Cast { value, .. } => walk(value, f),
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                walk(b, f);
            }
            if let Some(e2) = end {
                walk(e2, f);
            }
        }
        // Leaves / variants this pass doesn't descend into.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectSet;
    use crate::expr::Literal;

    fn sym(s: &str) -> Symbol {
        Symbol::from(s)
    }
    fn lit_int(n: i64) -> Expr {
        syn(ExprNode::Lit { value: Literal::Int { value: n } })
    }
    fn nil() -> Expr {
        syn(ExprNode::Lit { value: Literal::Nil })
    }
    fn seq(exprs: Vec<Expr>) -> Expr {
        syn(ExprNode::Seq { exprs })
    }
    fn assign(name: &str, value: Expr) -> Expr {
        syn(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: sym(name) },
            value,
        })
    }
    fn index_assign(recv: &str, key: Expr, value: Expr) -> Expr {
        syn(ExprNode::Assign {
            target: LValue::Index { recv: var(&sym(recv)), index: key },
            value,
        })
    }
    fn ret(value: Expr) -> Expr {
        syn(ExprNode::Return { value })
    }
    fn if_(cond: Expr, then_branch: Expr, else_branch: Expr) -> Expr {
        syn(ExprNode::If { cond, then_branch, else_branch })
    }
    fn step_add(name: &str) -> Expr {
        syn(ExprNode::OpAssign {
            target: LValue::Var { id: VarId(0), name: sym(name) },
            op: OpAssignOp::Add,
            value: lit_int(1),
        })
    }
    fn while_(cond: Expr, body: Expr) -> Expr {
        syn(ExprNode::While { cond, body, until_form: false })
    }
    fn vr(name: &str) -> Expr {
        var(&sym(name))
    }
    fn method(name: &str, receiver: MethodReceiver, params: &[&str], body: Expr) -> MethodDef {
        MethodDef {
            name: sym(name),
            receiver,
            params: params.iter().map(|p| Param::positional(sym(p))).collect(),
            block_param: None,
            body,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        }
    }

    /// `def self.find(table); i = 0; while i < table.length; x = table[i];
    ///  return x if x == 1; i += 1; end; nil; end`
    fn find_method() -> MethodDef {
        let cond = binop(vr("i"), "<", binop_call(vr("table"), "length"));
        let loop_body = seq(vec![
            assign("x", index_get("table", vr("i"))),
            if_(binop(vr("x"), "==", lit_int(1)), ret(vr("x")), nil()),
            step_add("i"),
        ]);
        let body = seq(vec![assign("i", lit_int(0)), while_(cond, loop_body), nil()]);
        method("find", MethodReceiver::Class, &["table"], body)
    }

    fn binop_call(recv: Expr, method: &str) -> Expr {
        syn(ExprNode::Send {
            recv: Some(recv),
            method: sym(method),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    }
    fn index_get(recv: &str, index: Expr) -> Expr {
        syn(ExprNode::Send {
            recv: Some(vr(recv)),
            method: sym("[]"),
            args: vec![index],
            block: None,
            parenthesized: false,
        })
    }

    fn has_while(e: &Expr) -> bool {
        count_loops(e) > 0
    }

    #[test]
    fn canonical_counter_loop_splits_into_entry_and_helper() {
        let out = transform_method(find_method());
        assert_eq!(out.len(), 2, "expected entry + helper");

        let entry = &out[0];
        let helper = &out[1];
        assert_eq!(entry.name.as_str(), "find");
        assert_eq!(helper.name.as_str(), "find__loop");

        // Neither emitted method may still contain a `while`.
        assert!(!has_while(&entry.body), "entry still has a loop");
        assert!(!has_while(&helper.body), "helper still has a loop");

        // Helper params = method params then carried locals (`i`).
        let params: Vec<&str> = helper.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(params, vec!["table", "i"]);

        // Entry ends in the initial call into the helper.
        let entry_stmts = seq_stmts(&entry.body).expect("entry body is a Seq");
        let last = entry_stmts.last().unwrap();
        assert!(
            matches!(&*last.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "find__loop"),
            "entry should end in a bareword call to find__loop, got {:?}",
            last.node
        );

        // Helper body is an `if cond do … else … end`, and recurses.
        assert!(matches!(&*helper.body.node, ExprNode::If { .. }), "helper body should be an If");
        assert!(count_recursive_calls(&helper.body, "find__loop") >= 1, "helper should recurse");
    }

    fn count_recursive_calls(e: &Expr, name: &str) -> usize {
        let mut n = 0;
        walk(e, &mut |x| {
            if let ExprNode::Send { recv: None, method, .. } = &*x.node {
                if method.as_str() == name {
                    n += 1;
                }
            }
        });
        n
    }

    #[test]
    fn transformed_loop_emits_valid_elixir_shape() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;

        let methods = transform_method(find_method());
        let class = LibraryClass {
            name: ClassId(sym("Finder")),
            is_module: true,
            parent: None,
            includes: vec![],
            methods,
            origin: None,
        };
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        eprintln!("--- emitted ---\n{ex}\n---------------");
        // No loop construct survives, the entry calls the helper, and the
        // helper recurses — all rendered through the real Elixir walker.
        assert!(!ex.contains("while"), "no while should remain:\n{ex}");
        assert!(ex.contains("def find("), "entry present");
        assert!(ex.contains("def find__loop("), "helper present");
        assert!(ex.contains("find__loop(table, i)"), "recursive call present:\n{ex}");
        assert!(!ex.contains("elixir2: unhandled"), "no unsupported-node stub:\n{ex}");
    }

    #[test]
    fn method_without_loop_is_unchanged() {
        let m = method("plain", MethodReceiver::Class, &["x"], seq(vec![ret(vr("x"))]));
        let out = transform_method(m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name.as_str(), "plain");
    }

    #[test]
    fn accumulator_mutation_bails() {
        // Same as find, but the body also does `acc[k] = v` — outside v1.
        let cond = binop(vr("i"), "<", binop_call(vr("table"), "length"));
        let loop_body = seq(vec![
            index_assign("acc", vr("i"), vr("i")),
            step_add("i"),
        ]);
        let body = seq(vec![
            assign("acc", syn(ExprNode::Hash { entries: vec![], kwargs: false })),
            assign("i", lit_int(0)),
            while_(cond, loop_body),
            vr("acc"),
        ]);
        let out = transform_method(method("build", MethodReceiver::Class, &["table"], body));
        assert_eq!(out.len(), 1, "accumulator loop should be left unchanged");
        assert!(has_while(&out[0].body));
    }

    #[test]
    fn instance_receiver_loop_bails() {
        let mut m = find_method();
        m.receiver = MethodReceiver::Instance;
        let out = transform_method(m);
        assert_eq!(out.len(), 1, "instance-method loop is deferred (mutation-threading)");
    }
}
