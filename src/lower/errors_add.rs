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
//! Any receiver spelling grounds: `errors` as a zero-arg send on
//! `self` (a model adding to its own errors during validation) or on
//! another expression (`record.errors.add(...)` from a controller —
//! lobsters' duplicate-comment guard). The rewrite keeps the receiver,
//! so both land on the same accumulator. A dynamic (non-symbol) field
//! still joins the residue ledger — the accumulator is an
//! `Array[String]`, which has no `add`, so on strict targets each such
//! site is a named per-target gap rather than a silent compile error
//! (`lower_residue` warning, pass `errors_add`). Adjacent
//! string-literal concats in the message (`"a " << "b"`, lobsters'
//! line-wrap idiom) fold to one literal first — a runtime `<<` on a
//! frozen literal is a hazard the bake sidesteps.
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

/// Fold `"a" << "b"` / `"a" + "b"` chains of string literals into one
/// literal (left-recursively, so multi-line wraps fold whole). Any
/// non-literal operand leaves the expression untouched.
fn fold_str_concat(e: Expr) -> Expr {
    let ExprNode::Send { recv: Some(l), method, args, block: None, .. } = &*e.node else {
        return e;
    };
    if !(method.as_str() == "<<" || method.as_str() == "+") || args.len() != 1 {
        return e;
    }
    let left = fold_str_concat(l.clone());
    let (ExprNode::Lit { value: Literal::Str { value: lv } },
         ExprNode::Lit { value: Literal::Str { value: rv } }) = (&*left.node, &*args[0].node)
    else {
        return e;
    };
    Expr::new(e.span, ExprNode::Lit { value: Literal::Str { value: format!("{lv}{rv}") } })
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
        ExprNode::Send { args, .. }
            if !args.is_empty()
                && args.len() <= 2
                && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
    );
    if !matches {
        let reason = match &*expr.node {
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
    let mut msg = args.next().map(fold_str_concat);
    // Rails' options spelling: `errors.add(:field, message: "…")`.
    // The kwargs hash IS the message carrier — unwrap a sole
    // `message:` entry; any other option set stays residue (put the
    // Send back — it was already moved out).
    let sole_message = matches!(
        msg.as_ref().map(|m| &*m.node),
        Some(ExprNode::Hash { entries, .. })
            if entries.len() == 1
                && matches!(&*entries[0].0.node,
                    ExprNode::Lit { value: Literal::Sym { value } }
                        if value.as_str() == "message")
    );
    if sole_message {
        let Some(ExprNode::Hash { entries, .. }) = msg.take().map(|m| *m.node) else {
            unreachable!()
        };
        msg = Some(fold_str_concat(entries.into_iter().next().unwrap().1));
    } else if matches!(msg.as_ref().map(|m| &*m.node), Some(ExprNode::Hash { .. })) {
        diags.push(residue(expr, "unrecognized arg shape"));
        *expr.node = ExprNode::Send {
            recv,
            method: Symbol::from("add"),
            args: vec![field_expr, msg.unwrap()],
            block: None,
            parenthesized: true,
        };
        return;
    }
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
