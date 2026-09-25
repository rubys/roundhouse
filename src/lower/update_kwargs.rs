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
use crate::expr::{BoolOpKind, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
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
            "column-writing `update`/`touch` left uninlined ({reason}) — the \
             hash-bag `update` drops association keys, and neither the bang \
             form nor `touch(:column)` has a runtime target"
        ),
    )
}

/// The kwargs-form `update`/`update!` send shape this pass inlines —
/// and `touch(:col, …)`, which is the same shape with the values
/// implied: every named column gets the current time, and the record's
/// own `touch` then stamps `updated_at` and writes the row. That is the
/// call-site lowering `Base#touch`'s header asks for (a shared
/// `touch(name)` would index-write through a variable key, which one
/// strict emitter cannot compile). lobsters stamps its read markers
/// this way on /newest and /comments (`@user&.touch(:last_read_newest_
/// story)`).
fn recognized_update(e: &Expr) -> bool {
    inline_parts(e).is_some()
}

/// `(column writes, finishing call)` for a recognized send: the
/// kwargs of an `update`, or each `touch` column paired with the
/// current time.
fn inline_parts(e: &Expr) -> Option<(Vec<(Symbol, Expr)>, &'static str)> {
    let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    match method.as_str() {
        "update" | "update!" => {
            if args.len() != 1 {
                return None;
            }
            let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
            if entries.is_empty() {
                return None;
            }
            let mut out = Vec::new();
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
                    return None;
                };
                out.push((value.clone(), v.clone()));
            }
            Some((out, if method.as_str() == "update!" { "save!" } else { "save" }))
        }
        // An implicit or `self` receiver in a model body is
        // `lower::column_ops`' — one owner per call shape.
        "touch" if !args.is_empty() && !matches!(e.node.as_ref(), ExprNode::Send { recv: Some(r), .. } if matches!(&*r.node, ExprNode::SelfRef)) => {
            let mut out = Vec::new();
            for a in args {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node else {
                    return None;
                };
                out.push((value.clone(), now(e.span)));
            }
            Some((out, "touch"))
        }
        _ => None,
    }
}

/// `ActiveSupport.db_now` — the instant `Base#touch` stamps
/// `updated_at` with (`fill_timestamps`) and the runtime's one clock
/// (`travel_to` moves it). `lower::column_ops` writes the implicit-self
/// form with it for the same reasons.
fn now(span: crate::span::Span) -> Expr {
    let mut now = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })),
            method: Symbol::from("db_now"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    now.ty = Some(Ty::Str);
    now
}

fn rewrite(expr: &mut Expr, models: &HashSet<ClassId>, diags: &mut Vec<Diagnostic>) {
    // The `try(:update!, …)`-desugared guarded form — `recv &&
    // recv.update!(…)` — rewrites as a UNIT, before child recursion
    // would turn the right operand into a Seq: a BoolOp operand is a
    // VALUE slot, and the writer sequence is a statement shape (the
    // ruby emitter renders a Seq newline-joined, which silently
    // unnests the guard — `save!` escaped it and crashed on nil).
    // Ground to an If, and bind the receiver ONCE: association
    // readers re-query per call, so assigning through the raw reader
    // would write one instance and save another, losing every write.
    // Value divergence vs `&&` (nil where the falsy operand was) is
    // the class and_return already accepts — and `try`'s own
    // nil-receiver value IS nil, so the If is the closer model.
    let guarded = matches!(
        &*expr.node,
        ExprNode::BoolOp { op: BoolOpKind::And, right, .. } if recognized_update(right)
    );
    if guarded {
        let (is_model, _, pure) = {
            let ExprNode::BoolOp { right, .. } = &*expr.node else { unreachable!() };
            send_gates(right, models)
        };
        // Gate failures fall through untouched: the Send arm below
        // sees the same gates and pushes the one residue note.
        if is_model && pure {
            let span = expr.span;
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::BoolOp { left, right, .. } = node else { unreachable!() };
            let (entries, finish) = inline_parts(&right).expect("recognized above");
            let ExprNode::Send { recv: Some(r), .. } = *right.node else { unreachable!() };
            let local = Symbol::from("__update_rcv");
            let bind = Expr::new(
                span,
                ExprNode::Assign {
                    target: LValue::Var { id: VarId(0), name: local.clone() },
                    value: r,
                },
            );
            let mut stmts = vec![bind];
            for (key, v) in entries {
                stmts.push(Expr::new(
                    span,
                    ExprNode::Assign {
                        target: LValue::Attr {
                            recv: Expr::new(
                                span,
                                ExprNode::Var { id: VarId(0), name: local.clone() },
                            ),
                            name: key,
                        },
                        value: v,
                    },
                ));
            }
            let mut save_call = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        span,
                        ExprNode::Var { id: VarId(0), name: local },
                    )),
                    method: Symbol::from(finish),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            save_call.ty = Some(Ty::Bool);
            stmts.push(save_call);
            *expr.node = ExprNode::If {
                cond: left,
                then_branch: Expr::new(span, ExprNode::Seq { exprs: stmts }),
                else_branch: Expr::new(span, ExprNode::Seq { exprs: vec![] }),
            };
            expr.ty = Some(Ty::Union { variants: vec![Ty::Bool, Ty::Nil] });
        }
    }
    expr.node.for_each_child_mut(&mut |c| rewrite(c, models, diags));
    if !recognized_update(expr) {
        return;
    }
    let (is_model, ty_unknown, pure) = send_gates(expr, models);
    if !is_model {
        // A receiver positively typed to a non-model (a Hash whose
        // `update` is `merge!`) is correct in source shape — no note.
        if ty_unknown {
            diags.push(residue(expr, "receiver not typed to a model"));
        }
        return;
    }
    let span = expr.span;
    let (entries, finish) = inline_parts(expr).expect("recognized above");
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv: Some(r), .. } = node else { unreachable!() };
    let mut exprs: Vec<Expr> = Vec::new();
    // Each write re-reads the receiver, so one that is not an
    // effect-free reader (`@comment.story` — an association read that
    // may query) is bound ONCE first, as the guarded form above does:
    // writing through two reads would set one instance and save
    // another.
    let r = if pure {
        r
    } else {
        let local = Symbol::from("__update_rcv");
        let ty = r.ty.clone();
        exprs.push(Expr::new(
            span,
            ExprNode::Assign { target: LValue::Var { id: VarId(0), name: local.clone() }, value: r },
        ));
        let mut v = Expr::new(span, ExprNode::Var { id: VarId(0), name: local });
        v.ty = ty;
        v
    };
    for (key, v) in entries {
        exprs.push(Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Attr { recv: r.clone(), name: key },
                value: v,
            },
        ));
    }
    let mut save_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(r),
            method: Symbol::from(finish),
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

/// The three per-site gates, read off a recognized update send:
/// receiver types to a model, receiver type is unknown (residue
/// note), receiver is an effect-free reader chain (safe to
/// re-evaluate once more for the guard/bind).
fn send_gates(e: &Expr, models: &HashSet<ClassId>) -> (bool, bool, bool) {
    let ExprNode::Send { recv: Some(r), .. } = &*e.node else { unreachable!() };
    (
        recv_is_model(r, models),
        recv_ty_is_unknown(r),
        super::blank::is_effect_free_reader(r),
    )
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
