//! Diagnostic walker: collect analyzer diagnostics across the app's typed
//! trees (IR-carried annotations + unresolved/gradual leaf detection) plus the
//! missing-preload coverage pass. Extracted verbatim from `src/analyze/mod.rs`
//! (pure code motion). `diagnose` / `diagnose_with_coverage` are re-exported
//! from `crate::analyze` so external call paths are unchanged.

use crate::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ty::Ty;

use super::PreloadCoverage;
use super::preload;


/// Walk an analyzed `App` collecting every position where typing failed
/// in a way that matters for downstream typed emission. Does not modify
/// the IR — purely a read pass.
///
/// Scope of what's reported:
/// - Ivar reads whose `ty` remained `Ty::Var(0)`.
/// - Send calls with a concrete receiver type whose method wasn't found.
///
/// Deliberately NOT reported (noise suppression):
/// - Bare-name Sends whose receiver is implicit-self / None. Views without
///   a self_ty call many helpers we don't model (e.g. `csrf_meta_tags`);
///   flagging each would drown real diagnostics. Once helpers land via
///   the dialect registry expansion, this filter can be relaxed.
/// - Sends whose receiver itself is unknown. The root cause is upstream;
///   reporting both duplicates signal.
pub fn diagnose(app: &App) -> Vec<Diagnostic> {
    diagnose_with_coverage(app).0
}

/// [`diagnose`] plus the missing-preload coverage triple, for report
/// skins that state the denominator (#64: "0 findings" must be
/// distinguishable from "couldn't check").
pub fn diagnose_with_coverage(app: &App) -> (Vec<Diagnostic>, PreloadCoverage) {
    let mut out = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            diagnose_expr(&action.body, &mut out);
        }
    }
    for model in &app.models {
        for scope in model.scopes() {
            diagnose_expr(&scope.body, &mut out);
        }
        for method in model.methods() {
            diagnose_expr(&method.body, &mut out);
        }
    }
    for view in &app.views {
        diagnose_expr(&view.body, &mut out);
    }
    if let Some(seeds) = &app.seeds {
        diagnose_expr(seeds, &mut out);
    }

    // Static N+1 pass (#64): missing-preload warnings over the typed
    // query chains, same-procedure and through the controller→view
    // ivar channel.
    let (preload_diags, coverage) = preload::missing_preload_report(app);
    out.extend(preload_diags);

    // Collapse diagnostics that render to the same place with the same
    // text — same start position, same kind, same message. Method chains
    // whose links share a (not-yet-precise) start each emit there, so
    // `a.b`, `a.b.c`, `a.b.c.d` stack 2-5 squiggles of differing length
    // but identical tooltip on one spot. Key on `start` (what line:col
    // and the squiggle's anchor derive from), not the full range, so the
    // nested links collapse. `retain` keeps the first — and since the
    // walker emits the outer node before recursing, that's the longest,
    // outermost span. Self-correcting: once span preservation gives links
    // distinct starts, they survive on their own again.
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| seen.insert((d.span.file, d.span.start, d.code(), d.message.clone())));
    (out, coverage)
}

/// A type is "unknown" if it's `None` or `Ty::Var(n)` (a placeholder the
/// analyzer set for positions it couldn't resolve). `Ty::Untyped` —
/// the gradual escape — counts as *known*: the author signed that
/// position out of checking.
fn is_unknown_ty(ty: Option<&Ty>) -> bool {
    match ty {
        None => true,
        Some(Ty::Var { .. }) => true,
        _ => false,
    }
}

/// Short label for what shape of expression resolved to `Untyped`.
/// Used for the `GradualUntyped` diagnostic message so a single
/// kind can name the syntactic position without each callsite
/// recomputing. Lowercase, grep-friendly.
fn expr_kind_label(expr: &Expr) -> &'static str {
    match &*expr.node {
        ExprNode::Send { .. } => "method call",
        ExprNode::Ivar { .. } => "ivar read",
        ExprNode::Var { .. } => "local read",
        ExprNode::Const { .. } => "constant read",
        ExprNode::Apply { .. } => "function call",
        ExprNode::Yield { .. } => "yield",
        _ => "expression",
    }
}

/// The identifier at an unresolved leaf position, for the
/// `UnresolvedType` message — the called method, read local, or
/// constant path. `None` for nameless positions (`yield`). An `Apply`
/// names its callee when that callee is itself a named leaf.
fn unresolved_name(expr: &Expr) -> Option<crate::ident::Symbol> {
    match &*expr.node {
        ExprNode::Send { method, .. } => Some(method.clone()),
        ExprNode::Var { name, .. } => Some(name.clone()),
        ExprNode::Const { path } => Some(crate::ident::Symbol::new(
            &path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        )),
        ExprNode::Apply { fun, .. } => unresolved_name(fun),
        _ => None,
    }
}

fn diagnose_expr(expr: &Expr, out: &mut Vec<Diagnostic>) {
    // Diagnostic annotations set by the body-typer during analyze.
    // These are the IR-carried path: detection happens once at the
    // point of typing, and every reader (including this walker) sees
    // the same set.
    if let Some(kind) = &expr.diagnostic {
        let message = match kind {
            DiagnosticKind::IncompatibleBinop { op, lhs_ty, rhs_ty } => {
                format!(
                    "`{}` with incompatible operand types: {lhs_ty:?} {} {rhs_ty:?}",
                    op.as_str(),
                    op.as_str()
                )
            }
            DiagnosticKind::IvarUnresolved { name } => {
                format!("@{} has no known type", name.as_str())
            }
            DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
                format!("no known method `{}` on {recv_ty:?}", method.as_str())
            }
            DiagnosticKind::GradualUntyped { expr_kind } => {
                format!("{} resolves to RBS `untyped` (gradual escape)", expr_kind.as_str())
            }
            DiagnosticKind::UnresolvedType { expr_kind, name } => {
                Diagnostic::unresolved_type_text(expr_kind, name.as_ref())
            }
            DiagnosticKind::Unsupported { target, construct, detail } => {
                let mut m = Diagnostic::unsupported_text(target.as_ref(), construct);
                if !detail.is_empty() {
                    m.push_str(": ");
                    m.push_str(detail);
                }
                m
            }
            // Parse diagnostics come from the ingest parse wrapper and
            // MissingPreload from the post-walk preload pass — neither
            // is carried as an `Expr.diagnostic` annotation; handled
            // defensively so the match stays exhaustive.
            DiagnosticKind::Parse { message } => format!("syntax error: {message}"),
            DiagnosticKind::MissingPreload { association, .. } => {
                format!("query does not preload :{}", association.as_str())
            }
            // Produced by `lower::apply_post_analyze_lowerings` as
            // returned lists, never as `Expr.diagnostic` annotations;
            // handled defensively so the match stays exhaustive.
            DiagnosticKind::BlankUnlowered { method, reason, .. } => {
                format!("`{}` left as dynamic dispatch ({})", method.as_str(), reason.as_str())
            }
            DiagnosticKind::LowerResidue { pass, construct, reason } => {
                format!(
                    "`{}` left unlowered by {} ({})",
                    construct.as_str(),
                    pass.as_str(),
                    reason.as_str()
                )
            }
        };
        out.push(Diagnostic {
            span: expr.span,
            kind: kind.clone(),
            severity: Diagnostic::default_severity(kind),
            message,
        });
    }

    // RBS-declared `untyped` reaches this site. Emit a GradualUntyped
    // warning so consumers can track gradual-escape coverage and so
    // strict-target emitters can elevate to Error at emit time. The
    // body-typer doesn't annotate `expr.diagnostic` for Untyped — the
    // walker is the natural place since every node's `.ty` already
    // carries the signal.
    if matches!(expr.ty.as_ref(), Some(Ty::Untyped)) {
        let kind = DiagnosticKind::GradualUntyped {
            expr_kind: crate::ident::Symbol::new(expr_kind_label(expr)),
        };
        out.push(Diagnostic {
            span: expr.span,
            severity: Diagnostic::default_severity(&kind),
            kind,
            message: format!(
                "{} resolves to RBS `untyped` (gradual escape)",
                expr_kind_label(expr)
            ),
        });
    }

    match &*expr.node {
        ExprNode::Ivar { name } => {
            if is_unknown_ty(expr.ty.as_ref()) {
                let kind = DiagnosticKind::IvarUnresolved { name: name.clone() };
                out.push(Diagnostic {
                    span: expr.span,
                    severity: Diagnostic::default_severity(&kind),
                    kind,
                    message: format!("@{} has no known type", name.as_str()),
                });
            }
        }
        ExprNode::Send { recv: Some(r), method, .. } => {
            if !is_unknown_ty(r.ty.as_ref()) && is_unknown_ty(expr.ty.as_ref()) {
                let recv_ty = r.ty.clone().unwrap_or_else(|| Ty::Var { var: crate::ident::TyVar(0) });
                let kind = DiagnosticKind::SendDispatchFailed {
                    method: method.clone(),
                    recv_ty: recv_ty.clone(),
                };
                out.push(Diagnostic {
                    span: expr.span,
                    severity: Diagnostic::default_severity(&kind),
                    kind,
                    message: format!(
                        "no known method `{}` on {:?}",
                        method.as_str(),
                        recv_ty,
                    ),
                });
            }
        }
        _ => {}
    }

    // Residual unresolved positions the specific checks above don't
    // cover — the "silently unresolved" set. The body-typer left these
    // as an open inference variable (`Ty::Var`) or never stamped a type
    // (`None`), but no diagnostic fires today, so they pass invisibly:
    //   - implicit-self sends (`controller_name`, recv: None)
    //   - bare local and constant reads
    //   - function applies and yields
    // Ivars are reported by IvarUnresolved; explicit-receiver sends with
    // a *known* receiver by SendDispatchFailed. An explicit receiver that
    // is itself unresolved is reported on the receiver node when we
    // recurse, so the outer send is skipped here to avoid double-counting
    // the same root cause.
    if is_unknown_ty(expr.ty.as_ref()) {
        let report = matches!(
            &*expr.node,
            ExprNode::Send { recv: None, .. }
                | ExprNode::Var { .. }
                | ExprNode::Const { .. }
                | ExprNode::Apply { .. }
                | ExprNode::Yield { .. }
        );
        if report {
            let label = crate::ident::Symbol::new(expr_kind_label(expr));
            let name = unresolved_name(expr);
            let message = Diagnostic::unresolved_type_text(&label, name.as_ref());
            let kind = DiagnosticKind::UnresolvedType { expr_kind: label, name };
            out.push(Diagnostic {
                span: expr.span,
                severity: Diagnostic::default_severity(&kind),
                kind,
                message,
            });
        }
    }

    // Recurse into children so we surface every unresolved position.
    match &*expr.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                diagnose_expr(r, out);
            }
            for a in args {
                diagnose_expr(a, out);
            }
            if let Some(b) = block {
                diagnose_expr(b, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                diagnose_expr(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                diagnose_expr(k, out);
                diagnose_expr(v, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    diagnose_expr(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            diagnose_expr(left, out);
            diagnose_expr(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            diagnose_expr(cond, out);
            diagnose_expr(then_branch, out);
            diagnose_expr(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            diagnose_expr(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    diagnose_expr(g, out);
                }
                diagnose_expr(&arm.body, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            diagnose_expr(value, out);
            diagnose_expr(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            diagnose_expr(body, out);
        }
        ExprNode::Apply { fun, args, block } => {
            diagnose_expr(fun, out);
            for a in args {
                diagnose_expr(a, out);
            }
            if let Some(b) = block {
                diagnose_expr(b, out);
            }
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            diagnose_expr(value, out);
            if let LValue::Attr { recv, .. } = target {
                diagnose_expr(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                diagnose_expr(recv, out);
                diagnose_expr(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                diagnose_expr(a, out);
            }
        }
        ExprNode::Raise { value } => diagnose_expr(value, out),
        ExprNode::Return { value } => diagnose_expr(value, out),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    diagnose_expr(a, out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            diagnose_expr(body, out);
            for rc in rescues {
                for c in &rc.classes {
                    diagnose_expr(c, out);
                }
                diagnose_expr(&rc.body, out);
            }
            if let Some(e) = else_branch {
                diagnose_expr(e, out);
            }
            if let Some(e) = ensure {
                diagnose_expr(e, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value { diagnose_expr(v, out); }
        }
        ExprNode::Splat { value } => diagnose_expr(value, out),
        ExprNode::MultiAssign { targets, value } => {
            diagnose_expr(value, out);
            for target in targets {
                if let LValue::Attr { recv, .. } = target {
                    diagnose_expr(recv, out);
                }
                if let LValue::Index { recv, index } = target {
                    diagnose_expr(recv, out);
                    diagnose_expr(index, out);
                }
            }
        }
        ExprNode::While { cond, body, .. } => {
            diagnose_expr(cond, out);
            diagnose_expr(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin { diagnose_expr(b, out); }
            if let Some(e) = end { diagnose_expr(e, out); }
        }
        ExprNode::Cast { value, .. } => diagnose_expr(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
    }
}
