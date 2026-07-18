//! Mailer class-side call idiom: Rails'
//! `BanNotification.notify(user, banner, reason)` is method_missing —
//! the class proxies to `new.notify(...)` and wraps delivery. Under
//! static resolution every public instance method of a mailer class
//! gets an explicit class-side wrapper:
//!
//!   def self.notify(user, banner, reason)
//!     new.notify(user, banner, reason)
//!   end
//!
//! Mailer classes are those whose parent chain (within the ingested
//! set) reaches ActionMailer::Base. Positional forwarding only: a
//! keyword parameter would forward positionally and mis-bind (the
//! known strict-target kwarg-forwarding trap — and wrong arity even
//! on Ruby), and a block would be dropped, so methods with either
//! stay unwrapped on the residue ledger.
//!
//! Structural pass — it synthesizes methods on `app.library_classes`
//! (where `app/mailers/*.rb` ingest) rather than rewriting bodies, so
//! it walks the classes directly instead of `for_each_hook_body`.
//! Runs on the post-analyze hook (`apply_post_analyze_lowerings`) so
//! every target's tree carries the wrappers.

use std::collections::BTreeSet;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

/// Synthesize class-side wrappers on every mailer class. Returns the
/// residue ledger: instance methods left unwrapped, with the reason.
pub fn apply_mailer_class_side(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Transitive parent-chain closure: ApplicationMailer names
    // ActionMailer::Base directly, BanNotification names
    // ApplicationMailer. Iterate to fixpoint since ingest order is
    // arbitrary.
    let mut mailers: BTreeSet<String> = BTreeSet::new();
    loop {
        let mut changed = false;
        for lc in app.library_classes.iter() {
            let name = lc.name.0.as_str();
            if mailers.contains(name) {
                continue;
            }
            if let Some(p) = &lc.parent {
                let ps = p.0.as_str();
                if ps == "ActionMailer::Base" || mailers.contains(ps) {
                    mailers.insert(name.to_string());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    if mailers.is_empty() {
        return diags;
    }

    for lc in app.library_classes.iter_mut() {
        if !mailers.contains(lc.name.0.as_str()) {
            continue;
        }
        let class_side: BTreeSet<&str> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == crate::dialect::MethodReceiver::Class)
            .map(|m| m.name.as_str())
            .collect();
        let mut wrappers: Vec<crate::dialect::MethodDef> = Vec::new();
        for m in &lc.methods {
            if m.receiver != crate::dialect::MethodReceiver::Instance
                || m.name.as_str() == "initialize"
                || class_side.contains(m.name.as_str())
            {
                continue;
            }
            if m.params.iter().any(|p| p.keyword) {
                diags.push(residue(m, "keyword parameters do not forward positionally"));
                continue;
            }
            if m.block_param.is_some() {
                diags.push(residue(m, "the wrapper cannot forward a block"));
                continue;
            }

            // Param and return types are known by construction — the
            // wrapper mirrors the wrapped method — so stamp what the
            // signature records: the residual-diagnostics audit walks
            // hook output (unlike the emit-time pass this replaces),
            // and unstamped sends on typed receivers read as dispatch
            // failures.
            let (param_tys, ret_ty): (Vec<Option<Ty>>, Option<Ty>) = match &m.signature {
                Some(Ty::Fn { params, ret, .. }) if params.len() == m.params.len() => (
                    params.iter().map(|p| Some(p.ty.clone())).collect(),
                    Some((**ret).clone()),
                ),
                _ => (vec![None; m.params.len()], None),
            };

            let span = m.body.span;
            let args: Vec<Expr> = m
                .params
                .iter()
                .zip(param_tys)
                .enumerate()
                .map(|(i, (p, ty))| {
                    let mut v = Expr::new(
                        span,
                        ExprNode::Var {
                            id: crate::ident::VarId(i as u32),
                            name: p.name.clone(),
                        },
                    );
                    v.ty = ty;
                    v
                })
                .collect();
            let mut new_call = Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("new"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            new_call.ty = Some(Ty::Class { id: lc.name.clone(), args: vec![] });
            let mut body = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(new_call),
                    method: m.name.clone(),
                    args,
                    block: None,
                    parenthesized: true,
                },
            );
            body.ty = ret_ty;
            // Clone the instance method wholesale (signature, effects,
            // kind all carry over), then swap receiver + body.
            let mut w = m.clone();
            w.receiver = crate::dialect::MethodReceiver::Class;
            w.body = body;
            wrappers.push(w);
        }
        lc.methods.extend(wrappers);
    }
    diags
}

fn residue(m: &crate::dialect::MethodDef, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "mailer_class_side",
        "mailer-instance-method",
        m.body.span,
        reason,
        format!(
            "mailer method `{}` gets no class-side wrapper ({reason}) — \
             class-side call sites will not resolve",
            m.name.as_str()
        ),
    )
}
