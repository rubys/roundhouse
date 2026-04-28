//! Cross-cutting helpers used by multiple sub-modules of
//! `controller_to_library`: ivar-scope discovery, action-name mapping,
//! and the generic `map_expr` rewrite kernel.

use std::collections::BTreeSet;

use crate::dialect::{Action, Controller, Filter, FilterKind};
use crate::expr::{Arm, Expr, ExprNode, InterpPart, LValue, RescueClause};
use crate::ident::Symbol;

/// Derive the `Views::*` submodule name from a controller's class name.
/// `ArticlesController` → `Articles`. Returns None when the name doesn't
/// follow the `*Controller` convention or strips down to "Application"
/// (which has no view module).
pub(super) fn views_module_name(controller: &Controller) -> Option<String> {
    let name = controller.name.0.as_str();
    let stem = name.strip_suffix("Controller")?;
    if stem.is_empty() || stem == "Application" {
        return None;
    }
    Some(stem.to_string())
}

/// Collect every ivar that this action sees in scope at render time:
/// each `@x = ...` assignment in the body itself, plus the same in
/// every filter target whose `only:`/`except:` filter applies to this
/// action. Source order is preserved (body first, then each fired
/// filter in declaration order); duplicates dropped.
pub(super) fn ivars_in_scope(
    controller: &Controller,
    action_name: &str,
    body: &Expr,
    privs: &[Action],
) -> Vec<Symbol> {
    let mut seen: BTreeSet<Symbol> = BTreeSet::new();
    let mut out: Vec<Symbol> = Vec::new();

    let push = |sym: Symbol, seen: &mut BTreeSet<Symbol>, out: &mut Vec<Symbol>| {
        if seen.insert(sym.clone()) {
            out.push(sym);
        }
    };

    let mut from_body: Vec<Symbol> = Vec::new();
    collect_assigned_ivars(body, &mut from_body);
    for s in from_body {
        push(s, &mut seen, &mut out);
    }

    let action_sym = Symbol::from(action_name);
    for filter in controller.filters() {
        if !matches!(filter.kind, FilterKind::Before) {
            continue;
        }
        if !filter_applies_to(filter, &action_sym) {
            continue;
        }
        let Some(target_action) = privs.iter().find(|p| p.name == filter.target) else {
            continue;
        };
        let mut from_filter: Vec<Symbol> = Vec::new();
        collect_assigned_ivars(&target_action.body, &mut from_filter);
        for s in from_filter {
            push(s, &mut seen, &mut out);
        }
    }

    out
}

/// True when `filter` applies to `action`, per its `only:` / `except:`
/// list. Empty `only` + empty `except` = applies to everything.
fn filter_applies_to(filter: &Filter, action: &Symbol) -> bool {
    if !filter.only.is_empty() {
        return filter.only.iter().any(|a| a == action);
    }
    if !filter.except.is_empty() {
        return !filter.except.iter().any(|a| a == action);
    }
    true
}

/// Walk `expr` collecting every ivar that appears on the LHS of an
/// `Assign`. Source-order, deduplication is done by the caller.
fn collect_assigned_ivars(expr: &Expr, out: &mut Vec<Symbol>) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, .. } = &*expr.node {
        out.push(name.clone());
    }
    walk_children(expr, &mut |c| collect_assigned_ivars(c, out));
}

/// Visit every direct child Expr of `expr`. Mirrors `map_expr`'s
/// traversal but in read-only form — used by passes that need to scan
/// the tree without rewriting it.
fn walk_children<F: FnMut(&Expr)>(expr: &Expr, f: &mut F) {
    match &*expr.node {
        ExprNode::Seq { exprs } => exprs.iter().for_each(f),
        ExprNode::If { cond, then_branch, else_branch } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprNode::Case { scrutinee, arms } => {
            f(scrutinee);
            for a in arms {
                if let Some(g) = a.guard.as_ref() {
                    f(g);
                }
                f(&a.body);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv.as_ref() {
                f(r);
            }
            args.iter().for_each(&mut *f);
            if let Some(b) = block.as_ref() {
                f(b);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            f(fun);
            args.iter().for_each(&mut *f);
            if let Some(b) = block.as_ref() {
                f(b);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            f(left);
            f(right);
        }
        ExprNode::Lambda { body, .. } => f(body),
        ExprNode::Assign { target, value } => {
            match target {
                LValue::Attr { recv, .. } => f(recv),
                LValue::Index { recv, index } => {
                    f(recv);
                    f(index);
                }
                _ => {}
            }
            f(value);
        }
        ExprNode::Array { elements, .. } => elements.iter().for_each(&mut *f),
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                f(k);
                f(v);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    f(expr);
                }
            }
        }
        ExprNode::Yield { args } => args.iter().for_each(&mut *f),
        ExprNode::Raise { value } => f(value),
        ExprNode::RescueModifier { expr, fallback } => {
            f(expr);
            f(fallback);
        }
        ExprNode::Return { value } => f(value),
        ExprNode::Super { args: Some(args) } => args.iter().for_each(&mut *f),
        ExprNode::Next { value: Some(v) } => f(v),
        ExprNode::Let { value, body, .. } => {
            f(value);
            f(body);
        }
        ExprNode::MultiAssign { value, .. } => f(value),
        ExprNode::While { cond, body, .. } => {
            f(cond);
            f(body);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin.as_ref() {
                f(b);
            }
            if let Some(e) = end.as_ref() {
                f(e);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            f(body);
            for r in rescues {
                r.classes.iter().for_each(&mut *f);
                f(&r.body);
            }
            if let Some(e) = else_branch.as_ref() {
                f(e);
            }
            if let Some(e) = ensure.as_ref() {
                f(e);
            }
        }
        _ => {}
    }
}

/// Action name → Ruby method name. `new` is the only rename (it
/// shadows `Object#new` if defined as an instance method; spinel's
/// router maps `:new` action to `new_action`).
pub(super) fn method_name_for_action(action: &str) -> &str {
    if action == "new" { "new_action" } else { action }
}

// ---------------------------------------------------------------------------
// Generic Expr rewrite helper. `f` runs on each node pre-order: when
// it returns `Some(replacement)`, the result is used verbatim (no
// further recursion into that subtree — `f` is responsible for
// recursing into children if needed). When it returns `None`, the
// default structural map runs, applying `map_expr` to every child.
//
// This is the small kernel that lets each rewriter (params,
// redirect_to, …) be a 10-line pattern match instead of a 130-line
// case-per-variant walker.
// ---------------------------------------------------------------------------

pub(super) fn map_expr<F>(expr: &Expr, f: &F) -> Expr
where
    F: Fn(&Expr) -> Option<Expr>,
{
    if let Some(replacement) = f(expr) {
        return replacement;
    }
    let new_node = match &*expr.node {
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|e| map_expr(e, f)).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: map_expr(cond, f),
            then_branch: map_expr(then_branch, f),
            else_branch: map_expr(else_branch, f),
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: map_expr(scrutinee, f),
            arms: arms
                .iter()
                .map(|a| Arm {
                    pattern: a.pattern.clone(),
                    guard: a.guard.as_ref().map(|g| map_expr(g, f)),
                    body: map_expr(&a.body, f),
                })
                .collect(),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(|r| map_expr(r, f)),
            method: method.clone(),
            args: args.iter().map(|a| map_expr(a, f)).collect(),
            block: block.as_ref().map(|b| map_expr(b, f)),
            parenthesized: *parenthesized,
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: map_expr(fun, f),
            args: args.iter().map(|a| map_expr(a, f)).collect(),
            block: block.as_ref().map(|b| map_expr(b, f)),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: map_expr(left, f),
            right: map_expr(right, f),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: map_expr(body, f),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: map_expr(recv, f),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: map_expr(recv, f),
                    index: map_expr(index, f),
                },
                other => other.clone(),
            };
            ExprNode::Assign { target: new_target, value: map_expr(value, f) }
        }
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(|e| map_expr(e, f)).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (map_expr(k, f), map_expr(v, f)))
                .collect(),
            braced: *braced,
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Expr { expr } => InterpPart::Expr { expr: map_expr(expr, f) },
                    other => other.clone(),
                })
                .collect(),
        },
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(|a| map_expr(a, f)).collect(),
        },
        ExprNode::Raise { value } => ExprNode::Raise { value: map_expr(value, f) },
        ExprNode::RescueModifier { expr, fallback } => ExprNode::RescueModifier {
            expr: map_expr(expr, f),
            fallback: map_expr(fallback, f),
        },
        ExprNode::Return { value } => ExprNode::Return { value: map_expr(value, f) },
        ExprNode::Super { args: Some(args) } => ExprNode::Super {
            args: Some(args.iter().map(|a| map_expr(a, f)).collect()),
        },
        ExprNode::Next { value: Some(v) } => ExprNode::Next { value: Some(map_expr(v, f)) },
        ExprNode::Let { name, id, value, body } => ExprNode::Let {
            name: name.clone(),
            id: *id,
            value: map_expr(value, f),
            body: map_expr(body, f),
        },
        ExprNode::MultiAssign { targets, value } => ExprNode::MultiAssign {
            targets: targets.clone(),
            value: map_expr(value, f),
        },
        ExprNode::While { cond, body, until_form } => ExprNode::While {
            cond: map_expr(cond, f),
            body: map_expr(body, f),
            until_form: *until_form,
        },
        ExprNode::Range { begin, end, exclusive } => ExprNode::Range {
            begin: begin.as_ref().map(|b| map_expr(b, f)),
            end: end.as_ref().map(|e| map_expr(e, f)),
            exclusive: *exclusive,
        },
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, implicit } => {
            ExprNode::BeginRescue {
                body: map_expr(body, f),
                rescues: rescues
                    .iter()
                    .map(|r| RescueClause {
                        classes: r.classes.iter().map(|c| map_expr(c, f)).collect(),
                        binding: r.binding.clone(),
                        body: map_expr(&r.body, f),
                    })
                    .collect(),
                else_branch: else_branch.as_ref().map(|e| map_expr(e, f)),
                ensure: ensure.as_ref().map(|e| map_expr(e, f)),
                implicit: *implicit,
            }
        }
        // Leaves (Lit / Var / Ivar / Const / SelfRef / Super{None} /
        // Next{None}) carry no children to rewrite.
        other => other.clone(),
    };
    Expr {
        span: expr.span,
        node: Box::new(new_node),
        ty: expr.ty.clone(),
        effects: expr.effects.clone(),
        leading_blank_line: expr.leading_blank_line,
        diagnostic: expr.diagnostic.clone(),
    }
}
