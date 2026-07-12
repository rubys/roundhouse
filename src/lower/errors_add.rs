//! `errors.add` grounding: `errors.add(:field, "msg")` →
//! `errors << "Field msg"`.
//!
//! The shared runtime's error accumulator is a plain `Array[String]` —
//! the validates lowering bakes humanized full messages at lower time
//! ("Short can't be blank") — so hand-written `add` calls ground into
//! the same shape. `:base` contributes the bare message (Rails
//! semantics); a dynamic message interpolates after the humanized
//! field; a missing message defaults to Rails' "is invalid".
//!
//! Only the self-receiver spelling grounds: `errors` must be a
//! zero-arg send on `self` (bare or explicit), i.e. a model adding to
//! its own errors during validation. `record.errors.add(...)` from the
//! outside keeps its dynamic call and joins the residue ledger — the
//! accumulator is an `Array[String]`, which has no `add`, so on strict
//! targets each such site is a named per-target gap rather than a
//! silent compile error (`lower_residue` warning, pass `errors_add`).
//!
//! Purely shape-directed; runs on the post-analyze hook
//! (`apply_post_analyze_lowerings`) with its siblings so every target
//! consumes the grounded form. Scope is `for_each_hook_body` (views
//! excluded like every hook pass — the construct has no view presence).

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;

/// Rewrite self-receiver `errors.add` sends across every hook body.
/// Returns the residue ledger: recognizable `errors.add` sites left
/// dynamic, with the reason.
pub fn apply_errors_add_lowering(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |body| rewrite_errors_add(body, &mut diags));
    diags
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("errors_add"),
        construct: Symbol::from("errors.add"),
        reason: Symbol::from(reason),
    };
    Diagnostic {
        span: expr.span,
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "`errors.add` left as dynamic dispatch ({reason}) — the error \
             accumulator is an Array[String] with no `add`; ground by hand \
             or extend the errors_add lowering"
        ),
    }
}

fn rewrite_errors_add(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite_errors_add(c, diags));
    // Any `add` on an `errors` reader is this pass's construct; decide
    // ground-vs-residue below.
    let recognized = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, block: None, .. }
            if method.as_str() == "add"
                && matches!(&*r.node, ExprNode::Send { method: em, args: ea, .. }
                    if em.as_str() == "errors" && ea.is_empty())
    );
    if !recognized {
        return;
    }
    let matches = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), args, .. }
            if !args.is_empty()
                && args.len() <= 2
                && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
                && matches!(&*r.node, ExprNode::Send { recv: er, .. }
                    if er.as_ref().is_none_or(
                        |e| matches!(&*e.node, ExprNode::SelfRef)))
    );
    if !matches {
        let reason = match &*expr.node {
            ExprNode::Send { recv: Some(r), .. }
                if matches!(&*r.node, ExprNode::Send { recv: Some(er), .. }
                    if !matches!(&*er.node, ExprNode::SelfRef)) =>
            {
                "non-self errors receiver"
            }
            ExprNode::Send { args, .. }
                if args.first().is_some_and(
                    |a| !matches!(&*a.node, ExprNode::Lit { value: Literal::Sym { .. } })) =>
            {
                "dynamic field"
            }
            _ => "unrecognized arg shape",
        };
        diags.push(residue(expr, reason));
        return;
    }
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, args, .. } = node else { unreachable!() };
    let mut args = args.into_iter();
    let field_expr = args.next().expect("checked non-empty");
    let ExprNode::Lit { value: Literal::Sym { value: field } } = &*field_expr.node else {
        unreachable!()
    };
    let msg = args.next();
    let humanized = super::model_to_library::validations::humanize(field.as_str());
    let message: Expr = if field.as_str() == "base" {
        // :base attaches the message to the record, not a field.
        msg.unwrap_or_else(|| {
            Expr::new(span, ExprNode::Lit { value: Literal::Str { value: "is invalid".into() } })
        })
    } else {
        match msg {
            Some(m) => match &*m.node {
                ExprNode::Lit { value: Literal::Str { value } } => Expr::new(
                    span,
                    ExprNode::Lit {
                        value: Literal::Str { value: format!("{humanized} {value}") },
                    },
                ),
                _ => Expr::new(
                    span,
                    ExprNode::StringInterp {
                        parts: vec![
                            InterpPart::Text { value: format!("{humanized} ") },
                            InterpPart::Expr { expr: m },
                        ],
                    },
                ),
            },
            None => Expr::new(
                span,
                ExprNode::Lit {
                    value: Literal::Str { value: format!("{humanized} is invalid") },
                },
            ),
        }
    };
    *expr.node = ExprNode::Send {
        recv,
        method: Symbol::from("<<"),
        args: vec![message],
        block: None,
        parenthesized: false,
    };
    expr.ty = None;
}
