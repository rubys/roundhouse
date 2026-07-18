//! Kwargs-form record update inlining: Rails'
//! `record.update!(k: v, ...)` / `record.update(k: v, ...)` with a
//! literal symbol-key hash becomes the writer-assign sequence
//!
//!   record.k = v
//!   ...
//!   record.save!            # bang form; plain form saves
//!
//! Two things make the inline load-bearing rather than cosmetic: the
//! synthesized model `update(attrs)` is a hash-bag over COLUMN keys
//! only — an association key (`new_user: nil`) would be silently
//! dropped — and no `update!` counterpart exists at all. The inline
//! form routes each key through its typed writer (column, temporal,
//! or belongs_to — the shared writer synthesis in
//! model_to_library::associations) and keeps bang semantics via
//! `save!`. The `Assign{Attr}` shape is the canonical writer-call IR.
//!
//! Type-gated: only receivers typed to an app model rewrite. Ruby's
//! `Hash#update` is `merge!`, so an unguarded pattern-match would
//! corrupt hash code; a receiver positively typed to something other
//! than a model is correct as-is and skips silently. A receiver whose
//! type is UNKNOWN goes on the residue ledger instead — if it is a
//! record at runtime, the plain form falls back to the hash-bag
//! `update` but the bang form has no target at all.
//!
//! Runs on the post-analyze hook (`apply_post_analyze_lowerings`) so
//! every target consumes the inlined form.

use std::collections::HashSet;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Inline kwargs-form `update`/`update!` sends across every hook body.
/// Returns the residue ledger: update-shaped calls left in source
/// shape, with the reason.
pub fn apply_update_kwargs_inline(app: &mut App) -> Vec<Diagnostic> {
    let models: HashSet<ClassId> = app.models.iter().map(|m| m.name.clone()).collect();
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |body| rewrite(body, &models, &mut diags));
    diags
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "update_kwargs_inline",
        "update-with-kwargs",
        expr.span,
        reason,
        format!(
            "kwargs-form `update` left uninlined ({reason}) — the hash-bag \
             fallback drops association keys and the bang form has no \
             runtime target"
        ),
    )
}

fn rewrite(expr: &mut Expr, models: &HashSet<ClassId>, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, models, diags));
    let recognized = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(_), method, args, block: None, .. }
            if matches!(method.as_str(), "update" | "update!")
                && args.len() == 1
                && matches!(&*args[0].node, ExprNode::Hash { entries, .. }
                    if !entries.is_empty() && entries.iter().all(|(k, _)| matches!(
                        &*k.node, ExprNode::Lit { value: Literal::Sym { .. } })))
    );
    if !recognized {
        return;
    }
    let (is_model, ty_unknown, pure) = {
        let ExprNode::Send { recv: Some(r), .. } = &*expr.node else { unreachable!() };
        (
            recv_is_model(r, models),
            recv_ty_is_unknown(r),
            super::blank::is_effect_free_reader(r),
        )
    };
    if !is_model {
        // A receiver positively typed to a non-model (a Hash whose
        // `update` is `merge!`) is correct in source shape — no note.
        if ty_unknown {
            diags.push(residue(expr, "receiver not typed to a model"));
        }
        return;
    }
    if !pure {
        // Each assigned key re-evaluates the receiver, so it must be
        // an effect-free reader chain.
        diags.push(residue(expr, "receiver is not an effect-free reader"));
        return;
    }

    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv: Some(r), method, args, .. } = node else { unreachable!() };
    let ExprNode::Hash { entries, .. } = &*args[0].node else { unreachable!() };
    let mut exprs: Vec<Expr> = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            unreachable!()
        };
        exprs.push(Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Attr { recv: r.clone(), name: key.clone() },
                value: v.clone(),
            },
        ));
    }
    let save = if method.as_str() == "update!" { "save!" } else { "save" };
    let mut save_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(r),
            method: Symbol::from(save),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    // The synthesized nodes are walked by the residual-diagnostics
    // audit (unlike the emit-time pass this replaces, which ran after
    // it) — stamp what the lowerer knows: save/save! return Bool, and
    // the Seq's value is that save result, matching `update`'s Bool.
    save_call.ty = Some(Ty::Bool);
    exprs.push(save_call);
    *expr.node = ExprNode::Seq { exprs };
    expr.ty = Some(Ty::Bool);
}

/// True when the receiver types to an app model — directly
/// (`Class{Invitation}`) or through the post-`find_by` nilable shape
/// (`Invitation | Nil`, whose nil arm crashes on `update` exactly as
/// Rails would).
fn recv_is_model(r: &Expr, models: &HashSet<ClassId>) -> bool {
    match r.ty.as_ref() {
        Some(Ty::Class { id, .. }) => models.contains(id),
        Some(Ty::Union { variants }) => {
            let mut class: Option<&ClassId> = None;
            for v in variants {
                match v {
                    Ty::Nil => {}
                    Ty::Class { id, .. } => {
                        if class.is_some() {
                            return false;
                        }
                        class = Some(id);
                    }
                    _ => return false,
                }
            }
            class.is_some_and(|id| models.contains(id))
        }
        _ => false,
    }
}

fn recv_ty_is_unknown(r: &Expr) -> bool {
    matches!(r.ty.as_ref(), None | Some(Ty::Untyped) | Some(Ty::Var { .. }))
}
