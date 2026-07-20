//! Inferred-type harvest: collect every type the body-typer stamped on the IR,
//! paired with its source span (browser-playground hover surface). Extracted
//! verbatim from `src/analyze/mod.rs` (pure code motion). `inferred_types` is
//! re-exported from `crate::analyze` so its public path is unchanged.
//!
//! NOTE: the parent plan suggested moving this "alongside ide.rs's consumers",
//! but ide.rs does not actually call it (the only match is a test name); moving
//! it under crate::ide would change the public path with no co-location gain, so
//! it lives in its own analyze submodule with a re-export instead.

use crate::App;
use crate::expr::{Expr, ExprNode, LValue};

/// Collect every inferred type the body-typer stamped on the IR, paired with
/// its source span. Mirrors [`diagnose`]'s roots + [`diagnose_expr`]'s
/// (exhaustive) recursion so coverage matches the diagnostics walk. Spans may
/// be synthetic or point at non-source (lowered) nodes — callers filter. Used
/// by the browser playground to surface inferred-type hovers.
pub fn inferred_types(app: &App) -> Vec<(crate::span::Span, crate::ty::Ty)> {
    let mut out = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            collect_types_expr(&action.body, &mut out);
        }
    }
    for model in &app.models {
        for scope in model.scopes() {
            collect_types_expr(&scope.body, &mut out);
        }
        for method in model.methods() {
            collect_types_expr(&method.body, &mut out);
        }
    }
    for view in &app.views {
        collect_types_expr(&view.body, &mut out);
    }
    if let Some(seeds) = &app.seeds {
        collect_types_expr(seeds, &mut out);
    }
    out
}

fn collect_types_expr(e: &Expr, out: &mut Vec<(crate::span::Span, crate::ty::Ty)>) {
    if let Some(ty) = &e.ty {
        out.push((e.span, ty.clone()));
    }
    // Recursion mirrors diagnose_expr exactly (exhaustive — a new ExprNode
    // variant breaks the build here too).
    match &*e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_types_expr(r, out);
            }
            for a in args {
                collect_types_expr(a, out);
            }
            if let Some(b) = block {
                collect_types_expr(b, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for x in exprs {
                collect_types_expr(x, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_types_expr(k, out);
                collect_types_expr(v, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    collect_types_expr(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            collect_types_expr(left, out);
            collect_types_expr(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_types_expr(cond, out);
            collect_types_expr(then_branch, out);
            collect_types_expr(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_types_expr(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_types_expr(g, out);
                }
                collect_types_expr(&arm.body, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            collect_types_expr(value, out);
            collect_types_expr(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            collect_types_expr(body, out);
        }
        ExprNode::Apply { fun, args, block } => {
            collect_types_expr(fun, out);
            for a in args {
                collect_types_expr(a, out);
            }
            if let Some(b) = block {
                collect_types_expr(b, out);
            }
        }
        ExprNode::Assign { target, value } | ExprNode::OpAssign { target, value, .. } => {
            collect_types_expr(value, out);
            if let LValue::Attr { recv, .. } = target {
                collect_types_expr(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                collect_types_expr(recv, out);
                collect_types_expr(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_types_expr(a, out);
            }
        }
        ExprNode::Raise { value } => collect_types_expr(value, out),
        ExprNode::Return { value } => collect_types_expr(value, out),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    collect_types_expr(a, out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_types_expr(body, out);
            for rc in rescues {
                for c in &rc.classes {
                    collect_types_expr(c, out);
                }
                collect_types_expr(&rc.body, out);
            }
            if let Some(x) = else_branch {
                collect_types_expr(x, out);
            }
            if let Some(x) = ensure {
                collect_types_expr(x, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                collect_types_expr(v, out);
            }
        }
        ExprNode::Splat { value } => collect_types_expr(value, out),
        ExprNode::MultiAssign { targets, value } => {
            collect_types_expr(value, out);
            for target in targets {
                if let LValue::Attr { recv, .. } = target {
                    collect_types_expr(recv, out);
                }
                if let LValue::Index { recv, index } = target {
                    collect_types_expr(recv, out);
                    collect_types_expr(index, out);
                }
            }
        }
        ExprNode::While { cond, body, .. } => {
            collect_types_expr(cond, out);
            collect_types_expr(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                collect_types_expr(b, out);
            }
            if let Some(x) = end {
                collect_types_expr(x, out);
            }
        }
        ExprNode::Cast { value, .. } => collect_types_expr(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
    }
}
