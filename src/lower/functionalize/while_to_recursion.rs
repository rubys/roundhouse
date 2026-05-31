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

    // Carried locals = vars bound in the pre-loop statements, in order,
    // that are referenced in the condition or loop body. (An accumulator
    // like `params = {}` is found here because `walk` descends into
    // index-assign targets, so `params[k] = v` counts as a reference.)
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

    // Thread accumulators: `acc[k] = v` on a carried `acc` becomes a
    // non-destructive rebind `acc = acc.merge({k => v})`, so the value
    // flows to the next iteration via the recursive call's by-name args.
    // (Index/field assignment to a *non*-carried var remains and trips
    // `has_unsupported` below — we can't thread what we don't carry.)
    let leading_rw: Vec<Expr> = leading.iter().map(|s| rewrite_accumulators(s, &carried)).collect();

    // Reject shapes outside v1: remaining accumulator/field mutation,
    // break/next, yield, or a second compound assignment (the trailing
    // counter step is excluded — we scan only `leading`).
    if leading_rw.iter().any(has_unsupported) {
        return None;
    }

    // --- build the two methods ---
    let helper_name = Symbol::from(format!("{}__loop", m.name.as_str()).as_str());

    // Helper params: the method's own params, then the carried locals.
    // Drop param defaults — the helper is always invoked with every
    // argument explicit (the entry call + each recursive tail pass all
    // of them by name), and a non-trailing default (e.g. `other \\ nil`
    // ahead of carried `keys`/`i`) is a compile error in Elixir.
    let mut helper_params: Vec<Param> =
        m.params.iter().map(|p| Param::positional(p.name.clone())).collect();
    helper_params.extend(carried.iter().map(|n| Param::positional(n.clone())));

    // An instance-method loop that touches `@ivar`/`self` threads the
    // record: `record` leads both the recursive call and (via the
    // emitter's instance-method threading) the helper's params. The
    // helper body's `@ivar` reads/writes are rewritten to `record.x` by
    // mutation_to_struct_return, which runs after this pass.
    let threads_record =
        m.receiver == MethodReceiver::Instance && touches_self(&m.body);

    // The recursive tail call passes every helper param by name; the
    // lowered counter step + accumulator rebinds (in scope at the call
    // site) advance the carried state.
    let mut recurse_args: Vec<Expr> = Vec::new();
    if threads_record {
        recurse_args.push(var(&Symbol::from("record")));
    }
    recurse_args.extend(helper_params.iter().map(|p| var(&p.name)));
    // A `yield` in the loop body becomes `block_fn.(…)`; thread `block_fn`
    // through the recursion (the emitter appends it as the trailing param
    // of both the entry and the helper).
    if contains_yield(while_body) {
        recurse_args.push(var(&Symbol::from("block_fn")));
    }
    let recurse_call = bareword_call(&helper_name, recurse_args);

    // Loop body (accumulators threaded) with the trailing step lowered.
    let mut body_stmts: Vec<Expr> = leading_rw;
    body_stmts.push(step_rebind);

    let helper = MethodDef {
        name: helper_name,
        // Same receiver as the entry: an instance-method loop's helper is
        // also an instance method, so mutation_to_struct_return threads
        // `record` through it and the emitter prepends the `record` param.
        receiver: m.receiver,
        params: helper_params,
        block_param: None,
        body: syn(ExprNode::If {
            cond: cond.clone(),
            then_branch: cps(&body_stmts, &recurse_call),
            // Post-loop value. A record-threading helper with nothing
            // after the loop (e.g. session#initialize's populate loop)
            // must yield the threaded `record` so the accumulated
            // struct flows back to the caller — not the `nil` that
            // falling off a Ruby `while` would produce.
            else_branch: if threads_record && post.is_empty() {
                var(&Symbol::from("record"))
            } else {
                value_of(post)
            },
        }),
        signature: None,
        effects: m.effects.clone(),
        enclosing_class: m.enclosing_class.clone(),
        kind: AccessorKind::Method,
        is_async: m.is_async,
        mutates_self: false,
    };

    // Entry: the pre-loop statements (which bind the carried locals),
    // ending in the initial call into the helper. CPS so a pre-loop
    // guard (`return X if c`) becomes an `if`, not a bare `return`.
    let mut entry = m.clone();
    entry.body = cps(pre, &recurse_call);

    Some(vec![entry, helper])
}

// ---- accumulator threading ------------------------------------------

/// Rewrite `acc[k] = v` (index assignment to a carried var) into the
/// non-destructive rebind `acc = acc.merge({k => v})`. Descends through
/// `Seq` and `If` branches (where loop-body statements live). Other
/// nodes are returned unchanged.
fn rewrite_accumulators(e: &Expr, carried: &[Symbol]) -> Expr {
    match &*e.node {
        // `acc[k] = v` rides the Send channel as `acc.[]=(k, v)`.
        ExprNode::Send { recv: Some(recv), method, args, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            if let ExprNode::Var { name, .. } = &*recv.node {
                if carried.contains(name) {
                    let entry = (args[0].clone(), args[1].clone());
                    let merged = binop(
                        var(name),
                        "merge",
                        syn(ExprNode::Hash { entries: vec![entry], kwargs: false }),
                    );
                    return syn(ExprNode::Assign {
                        target: LValue::Var { id: VarId(0), name: name.clone() },
                        value: merged,
                    });
                }
            }
            e.clone()
        }
        ExprNode::Seq { exprs } => syn(ExprNode::Seq {
            exprs: exprs.iter().map(|x| rewrite_accumulators(x, carried)).collect(),
        }),
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: cond.clone(),
            then_branch: rewrite_accumulators(then_branch, carried),
            else_branch: rewrite_accumulators(else_branch, carried),
        }),
        _ => e.clone(),
    }
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
        // A `[]=` on a non-carried local can't be threaded. A carried
        // `acc.[]=` was already rewritten to a merge rebind, and an
        // ivar-rooted `@x[k] = v` is threaded later by
        // mutation_to_struct_return — both are allowed here.
        ExprNode::Send { recv: Some(r), method, .. }
            if method.as_str() == "[]=" && !matches!(&*r.node, ExprNode::Ivar { .. }) =>
        {
            bad = true
        }
        ExprNode::Assign { target: LValue::Index { .. } | LValue::Attr { .. }, .. }
        | ExprNode::OpAssign { target: LValue::Index { .. } | LValue::Attr { .. }, .. }
        // A compound assignment other than the (already-excluded) trailing step.
        | ExprNode::OpAssign { target: LValue::Var { .. }, .. }
        // Loop control / nested loops. (`yield` IS supported — it threads
        // `block_fn` through the recursion.)
        | ExprNode::Break { .. }
        | ExprNode::Next { .. }
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

/// True when the body contains a `yield` — the signal to thread `block_fn`.
fn contains_yield(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| {
        if matches!(&*n.node, ExprNode::Yield { .. }) {
            found = true;
        }
    });
    found
}

/// True when the method body reads/writes instance state (`@ivar`/`self`)
/// — the signal that an instance-method loop must thread `record`.
fn touches_self(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| {
        if matches!(&*n.node, ExprNode::Ivar { .. } | ExprNode::SelfRef) {
            found = true;
        }
    });
    found
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
        ExprNode::Assign { target, value } => {
            walk_lvalue(target, f);
            walk(value, f);
        }
        ExprNode::OpAssign { target, value, .. } => {
            walk_lvalue(target, f);
            walk(value, f);
        }
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

/// Walk the sub-expressions inside an assignment target (`a[k]`, `o.x`).
fn walk_lvalue(lv: &LValue, f: &mut impl FnMut(&Expr)) {
    match lv {
        LValue::Index { recv, index } => {
            walk(recv, f);
            walk(index, f);
        }
        LValue::Attr { recv, .. } => walk(recv, f),
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
        // `recv[key] = value` rides the Send channel as `recv.[]=(key, value)`.
        syn(ExprNode::Send {
            recv: Some(vr(recv)),
            method: sym("[]="),
            args: vec![key, value],
            block: None,
            parenthesized: false,
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

    fn str_lit(s: &str) -> Expr {
        syn(ExprNode::Lit { value: Literal::Str { value: s.to_string() } })
    }
    fn call1(recv: Expr, method: &str, arg: Expr) -> Expr {
        syn(ExprNode::Send {
            recv: Some(recv),
            method: sym(method),
            args: vec![arg],
            block: None,
            parenthesized: false,
        })
    }

    /// `def self.mp(pattern, path); parts = pattern.split("/");
    ///  return nil if parts.length == 0; params = {}; i = 0;
    ///  while i < parts.length; pp = parts[i];
    ///    if pp.start_with?(":"); params[pp] = pp; end; i += 1; end; params; end`
    fn match_pattern_method() -> MethodDef {
        let cond = binop(vr("i"), "<", binop_call(vr("parts"), "length"));
        let acc = if_(
            call1(vr("pp"), "start_with?", str_lit(":")),
            index_assign("params", vr("pp"), vr("pp")),
            nil(),
        );
        let loop_body = seq(vec![
            assign("pp", index_get("parts", vr("i"))),
            acc,
            step_add("i"),
        ]);
        let pre_guard = if_(
            binop(binop_call(vr("parts"), "length"), "==", lit_int(0)),
            ret(nil()),
            nil(),
        );
        let body = seq(vec![
            assign("parts", call1(vr("pattern"), "split", str_lit("/"))),
            pre_guard,
            assign("params", syn(ExprNode::Hash { entries: vec![], kwargs: false })),
            assign("i", lit_int(0)),
            while_(cond, loop_body),
            vr("params"),
        ]);
        method("mp", MethodReceiver::Class, &["pattern", "path"], body)
    }

    #[test]
    fn pre_loop_guard_and_accumulator_are_handled() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;

        let out = transform_method(match_pattern_method());
        assert_eq!(out.len(), 2);
        let helper = &out[1];
        assert_eq!(helper.name.as_str(), "mp__loop");

        // The accumulator is carried (a helper param) and threaded — no
        // index-assignment survives in the helper body.
        let params: Vec<&str> = helper.params.iter().map(|p| p.name.as_str()).collect();
        assert!(params.contains(&"params"), "accumulator must be carried: {params:?}");
        assert!(params.contains(&"i") && params.contains(&"parts"));
        let mut index_assigns = 0;
        walk(&helper.body, &mut |n| {
            if matches!(&*n.node, ExprNode::Assign { target: LValue::Index { .. }, .. }) {
                index_assigns += 1;
            }
        });
        assert_eq!(index_assigns, 0, "index-assign should be threaded to a merge rebind");

        // Render through the real Elixir walker.
        let class = LibraryClass {
            name: ClassId(sym("Pat")),
            is_module: true,
            parent: None,
            includes: vec![],
            methods: out,
            origin: None,
        };
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        eprintln!("--- match_pattern ---\n{ex}\n---------------------");
        assert!(!ex.contains("while"), "no while:\n{ex}");
        assert!(ex.contains("Map.merge(params,"), "accumulator merge:\n{ex}");
        assert!(ex.contains("mp__loop("), "recurses:\n{ex}");
        assert!(!ex.contains("elixir2: unhandled"), "no unsupported stub:\n{ex}");
        // The pre-loop guard became an `if`, not a bare `return`.
        assert!(!ex.contains("return"), "no bare return:\n{ex}");
    }

    #[test]
    fn list_receiver_uses_enum_and_kernel_length() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;
        use crate::ty::Ty;

        // `def self.first(table); table.length; end` with `table: Array`.
        let mut table = vr("table");
        table.ty = Some(Ty::Array { elem: Box::new(Ty::Untyped) });
        let body = seq(vec![binop_call(table, "length")]);
        let m = method("len", MethodReceiver::Class, &["table"], body);
        let class = LibraryClass {
            name: ClassId(sym("L")),
            is_module: true,
            parent: None,
            includes: vec![],
            methods: vec![m],
            origin: None,
        };
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        assert!(ex.contains("length(table)"), "list length via Kernel.length:\n{ex}");
        assert!(!ex.contains("String.length(table)"), "should not use String.length:\n{ex}");
    }

    #[test]
    fn instance_method_loop_threads_record() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;
        // def fill(n)            # Instance
        //   i = 0
        //   while i < n
        //     @data[i] = i
        //     i += 1
        //   end
        //   self
        // end
        let loop_body = seq(vec![
            index_assign_ivar("data", vr("i"), vr("i")),
            step_add("i"),
        ]);
        let body = seq(vec![
            assign("i", lit_int(0)),
            while_(binop(vr("i"), "<", vr("n")), loop_body),
            syn(ExprNode::SelfRef),
        ]);
        let m = method("fill", MethodReceiver::Instance, &["n"], body);
        // Run the full functionalize pipeline (while→recursion, then
        // mutation-threading) and render.
        let class = LibraryClass {
            name: ClassId(sym("S")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods: vec![m],
            origin: None,
        };
        let class = crate::lower::functionalize::functionalize(vec![class]).pop().unwrap();
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        eprintln!("--- instance loop ---\n{ex}\n---------------------");
        assert!(ex.contains("def fill(record, n)"), "entry threads record:\n{ex}");
        assert!(ex.contains("fill__loop(record, n, i)"), "initial call passes record:\n{ex}");
        assert!(ex.contains("def fill__loop(record, n, i)"), "helper threads record:\n{ex}");
        assert!(ex.contains("record = %{record | data: Map.put(record.data, i, i)}"), "nested @data in loop:\n{ex}");
        assert!(!ex.contains("while"), "no while:\n{ex}");
    }

    #[test]
    fn instance_loop_with_yield_threads_record_and_block_fn() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;
        // def each            # Instance
        //   keys = @data.keys
        //   i = 0
        //   while i < keys.length
        //     k = keys[i]
        //     yield k          # (simplified: yield one value)
        //     i += 1
        //   end
        //   self
        // end
        let ivar_keys = syn(ExprNode::Send {
            recv: Some(syn(ExprNode::Ivar { name: sym("data") })),
            method: sym("keys"),
            args: vec![],
            block: None,
            parenthesized: false,
        });
        let loop_body = seq(vec![
            assign("k", index_get("keys", vr("i"))),
            syn(ExprNode::Yield { args: vec![vr("k")] }),
            step_add("i"),
        ]);
        let body = seq(vec![
            assign("keys", ivar_keys),
            assign("i", lit_int(0)),
            while_(binop(vr("i"), "<", binop_call(vr("keys"), "length")), loop_body),
            syn(ExprNode::SelfRef),
        ]);
        let class = LibraryClass {
            name: ClassId(sym("Sess")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods: vec![method("each", MethodReceiver::Instance, &[], body)],
            origin: None,
        };
        let class = crate::lower::functionalize::functionalize(vec![class]).pop().unwrap();
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        eprintln!("--- each w/ yield ---\n{ex}\n---------------------");
        assert!(ex.contains("def each(record, block_fn)"), "entry threads record+block_fn:\n{ex}");
        assert!(ex.contains("each__loop(record, keys, i, block_fn)"), "recurse threads both:\n{ex}");
        assert!(ex.contains("def each__loop(record, keys, i, block_fn)"), "helper params:\n{ex}");
        assert!(ex.contains("block_fn.(k)"), "yield → block_fn call:\n{ex}");
        assert!(!ex.contains("while"), "no while:\n{ex}");
    }

    fn index_assign_ivar(ivar: &str, key: Expr, value: Expr) -> Expr {
        syn(ExprNode::Send {
            recv: Some(syn(ExprNode::Ivar { name: sym(ivar) })),
            method: sym("[]="),
            args: vec![key, value],
            block: None,
            parenthesized: false,
        })
    }

    #[test]
    fn constructor_with_guard_and_populate_loop_threads_record() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;
        // Mirrors session#initialize:
        //   def initialize(other = nil)
        //     @data = {}
        //     return if other.nil?
        //     keys = other.keys
        //     i = 0
        //     while i < keys.length
        //       @data[keys[i]] = other[keys[i]]
        //       i += 1
        //     end
        //   end
        // plus Hash-backed read methods (`key?`, `delete`) that must
        // route to `Map.*` once `@data` is recognized as a Hash field.
        let other_keys = binop_call(vr("other"), "keys");
        let other_nil = binop_call(vr("other"), "nil?");
        let loop_body = seq(vec![
            index_assign_ivar("data", index_get("keys", vr("i")), index_get("other", vr("i"))),
            step_add("i"),
        ]);
        let init_body = seq(vec![
            assign_ivar_hash("data"),
            if_(other_nil, ret(nil()), nil()),
            assign("keys", other_keys),
            assign("i", lit_int(0)),
            while_(binop(vr("i"), "<", binop_call(vr("keys"), "length")), loop_body),
        ]);
        let mut initialize = method("initialize", MethodReceiver::Instance, &[], init_body);
        initialize.params = vec![Param::with_default(sym("other"), nil())];

        let key_q = method(
            "key?",
            MethodReceiver::Instance,
            &["key"],
            ivar_method_call("data", "key?", vec![vr("key")]),
        );
        let del = method(
            "delete",
            MethodReceiver::Instance,
            &["key"],
            ivar_method_call("data", "delete", vec![vr("key")]),
        );

        let class = LibraryClass {
            name: ClassId(sym("Session")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods: vec![initialize, key_q, del],
            origin: None,
        };
        let class = crate::lower::functionalize::functionalize(vec![class]).pop().unwrap();
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        eprintln!("--- constructor+loop ---\n{ex}\n------------------------");

        // Constructor: the guard `if` binds back to `record`, the nil
        // (bare-return-self) branch yields the unchanged `record`, and
        // the trailing `record` returns it.
        assert!(ex.contains("def new(other \\\\ nil)"), "new keeps its default:\n{ex}");
        assert!(
            ex.contains("record = if is_nil(other) do\n      record\n    else"),
            "guard binds to record, nil→record:\n{ex}"
        );
        // Helper carries NO param default (a mid-list `other \\ nil`
        // would be an Elixir compile error) and returns `record` after
        // the loop.
        assert!(
            ex.contains("def initialize__loop(record, other, keys, i)"),
            "helper params have no default:\n{ex}"
        );
        assert!(!ex.contains("other \\\\ nil, keys"), "no mid-list default:\n{ex}");
        // Hash-field reads route to Map.* (field typed Hash via the
        // `@data = {}` detection).
        assert!(ex.contains("Map.has_key?(record.data, key)"), "key? → Map.has_key?:\n{ex}");
        assert!(ex.contains("Map.delete(record.data, key)"), "delete → Map.delete:\n{ex}");
        assert!(!ex.contains("while"), "no while:\n{ex}");
    }

    fn assign_ivar_hash(name: &str) -> Expr {
        syn(ExprNode::Assign {
            target: LValue::Ivar { name: sym(name) },
            value: syn(ExprNode::Hash { entries: vec![], kwargs: false }),
        })
    }

    fn ivar_method_call(ivar: &str, method: &str, args: Vec<Expr>) -> Expr {
        syn(ExprNode::Send {
            recv: Some(syn(ExprNode::Ivar { name: sym(ivar) })),
            method: sym(method),
            args,
            block: None,
            parenthesized: false,
        })
    }

    #[test]
    fn method_without_loop_is_unchanged() {
        let m = method("plain", MethodReceiver::Class, &["x"], seq(vec![ret(vr("x"))]));
        let out = transform_method(m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name.as_str(), "plain");
    }

    #[test]
    fn carried_accumulator_threads() {
        // `acc = {}; while …; acc[i] = i; i += 1; end; acc` — the carried
        // accumulator is threaded (no longer bailed).
        let cond = binop(vr("i"), "<", binop_call(vr("table"), "length"));
        let loop_body = seq(vec![index_assign("acc", vr("i"), vr("i")), step_add("i")]);
        let body = seq(vec![
            assign("acc", syn(ExprNode::Hash { entries: vec![], kwargs: false })),
            assign("i", lit_int(0)),
            while_(cond, loop_body),
            vr("acc"),
        ]);
        let out = transform_method(method("build", MethodReceiver::Class, &["table"], body));
        assert_eq!(out.len(), 2, "carried accumulator loop should transform");
        assert!(!has_while(&out[1].body));
    }

    #[test]
    fn noncarried_index_write_bails() {
        // `other[i] = i` where `other` is not a carried (pre-loop-bound)
        // var — can't be threaded, so the loop is left unchanged.
        let cond = binop(vr("i"), "<", binop_call(vr("table"), "length"));
        let loop_body = seq(vec![index_assign("other", vr("i"), vr("i")), step_add("i")]);
        let body = seq(vec![assign("i", lit_int(0)), while_(cond, loop_body), nil()]);
        let out = transform_method(method("mut", MethodReceiver::Class, &["table", "other"], body));
        assert_eq!(out.len(), 1, "non-carried index write should be left unchanged");
        assert!(has_while(&out[0].body));
    }

    #[test]
    fn instance_receiver_loop_transforms() {
        let mut m = find_method();
        m.receiver = MethodReceiver::Instance;
        let out = transform_method(m);
        // Instance-method loops now transform (entry + helper); the helper
        // inherits the Instance receiver so record can thread through it.
        assert_eq!(out.len(), 2, "instance-method loop transforms");
        assert_eq!(out[1].receiver, MethodReceiver::Instance);
    }
}
