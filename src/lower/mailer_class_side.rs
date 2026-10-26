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

/// Every mailer class: transitive parent-chain closure, since
/// ApplicationMailer names ActionMailer::Base directly, BanNotification
/// names ApplicationMailer, and ingest order is arbitrary.
pub(crate) fn mailer_names(app: &App) -> BTreeSet<String> {
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
    mailers
}

/// Synthesize class-side wrappers on every mailer class. Returns the
/// residue ledger: instance methods left unwrapped, with the reason.
pub fn apply_mailer_class_side(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    let mailers = mailer_names(app);
    if mailers.is_empty() {
        return diags;
    }

    let parameterized = parameterized_mailers(app, &mailers);
    let own_with = defines_with(app);
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
            // ActionMailer's class-side proxy answers only the public
            // action methods (`action_methods`): a private or protected
            // helper or filter is never reachable as `Mailer.name`.
            if m.receiver != crate::dialect::MethodReceiver::Instance
                || m.visibility != crate::dialect::MethodVisibility::Public
                || m.name.as_str() == "initialize"
                || class_side.contains(m.name.as_str())
            {
                continue;
            }
            if m.params.iter().any(|p| p.forwarding) {
                diags.push(Diagnostic::unsupported(m.name_span, None, "full forwarding mailer wrapper",
                    "the generated mailer wrapper cannot preserve a full forwarding packet"));
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
                    params.iter().map(|p| Some((*p.ty).clone())).collect(),
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
            new_call.ty = Some(Ty::Class { id: lc.name.clone(), args: vec![].into() });
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
        if parameterized.contains(lc.name.0.as_str()) && !own_with.contains(lc.name.0.as_str()) {
            wrappers.push(parameterized_constructor(&lc.name));
        }
        lc.methods.extend(wrappers);
    }
    diags
}

/// Mailers whose own or an ancestor's `def self.with` answers `with`; Ruby would pick a generated one over the inherited.
pub(crate) fn defines_with(app: &App) -> BTreeSet<String> {
    let parents: std::collections::HashMap<&str, &str> = app
        .library_classes
        .iter()
        .filter_map(|lc| lc.parent.as_ref().map(|p| (lc.name.0.as_str(), p.0.as_str())))
        .collect();
    let own: BTreeSet<&str> = app
        .library_classes
        .iter()
        .filter(|lc| lc.methods.iter().any(|m| m.receiver == crate::dialect::MethodReceiver::Class && m.name.as_str() == "with"))
        .map(|lc| lc.name.0.as_str())
        .collect();
    let mut out = BTreeSet::new();
    for lc in &app.library_classes {
        let mut cur = Some(lc.name.0.as_str());
        for _ in 0..32 {
            let Some(name) = cur else { break };
            if own.contains(name) {
                out.insert(lc.name.0.as_str().to_string());
                break;
            }
            cur = parents.get(name).copied();
        }
    }
    out
}

// Not on every mailer: a target without the ActionMailer runtime has no `_with_params` to call.
fn parameterized_mailers(app: &App, mailers: &BTreeSet<String>) -> BTreeSet<String> {
    fn visit(e: &Expr, mailers: &BTreeSet<String>, out: &mut BTreeSet<String>) {
        if let ExprNode::Send { recv: Some(recv), method, .. } = &*e.node
            && method.as_str() == "with"
        {
            let name = match (&*recv.node, &recv.ty) {
                (ExprNode::Const { path }, _) => Some(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::")),
                (_, Some(Ty::Class { id, .. })) => Some(id.0.as_str().to_string()),
                _ => None,
            };
            if let Some(name) = name.filter(|n| mailers.contains(n)) {
                out.insert(name);
            }
        }
        e.node.for_each_child(&mut |c| visit(c, mailers, out));
    }
    let mut out = BTreeSet::new();
    for model in &app.models {
        model.methods().for_each(|m| visit(&m.body, mailers, &mut out));
    }
    for controller in &app.controllers {
        controller.actions().for_each(|a| visit(&a.body, mailers, &mut out));
        controller.class_methods().for_each(|m| visit(&m.body, mailers, &mut out));
    }
    for view in &app.views {
        visit(&view.body, mailers, &mut out);
    }
    for lc in &app.library_classes {
        lc.methods.iter().for_each(|m| visit(&m.body, mailers, &mut out));
    }
    if let Some(seeds) = &app.seeds {
        visit(seeds, mailers, &mut out);
    }
    out
}

// Not Rails' proxy class: the class-side wrappers already build the mail eagerly, so `with` hands back the mailer itself.
fn parameterized_constructor(mailer: &crate::ident::ClassId) -> crate::dialect::MethodDef {
    let span = crate::span::Span::synthetic();
    let instance = Ty::Class { id: mailer.clone(), args: vec![].into() };
    let params_ty = Ty::Hash { key: Ty::Sym.into(), value: Ty::Untyped.into() };
    let params = Symbol::from("params");
    let mut arg = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: params.clone() });
    arg.ty = Some(params_ty.clone());
    let mut new_call = Expr::new(
        span,
        ExprNode::Send { recv: None, method: Symbol::from("new"), args: vec![], block: None, parenthesized: false },
    );
    new_call.ty = Some(instance.clone());
    let mut body = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(new_call),
            method: Symbol::from("_with_params"),
            args: vec![arg],
            block: None,
            parenthesized: true,
        },
    );
    body.ty = Some(instance.clone());
    crate::dialect::MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: span,
        name: Symbol::from("with"),
        receiver: crate::dialect::MethodReceiver::Class,
        params: vec![crate::dialect::Param::positional(params.clone())],
        body,
        signature: Some(Ty::Fn {
            params: vec![crate::ty::Param { name: params, ty: params_ty.into(), kind: crate::ty::ParamKind::Required }].into(),
            block: None,
            ret: instance.into(),
            effects: Default::default(),
        }),
        effects: Default::default(),
        enclosing_class: Some(mailer.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
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
