//! `<recv>.config.session_options[:key]` → `<recv>.session_cookie_key`.
//!
//! Rails exposes the session cookie name as one entry in a heterogeneous
//! options bag: `Rails.application.config.session_options` also carries
//! `expire_after` (a Duration), `httponly` (a bool) and `same_site` (a
//! Symbol). Modelling the bag itself would mean a `Hash[Symbol, untyped]`
//! on the config surface, and every read off it would come back untyped —
//! which is exactly what blocks spinel AOT today, where lobsters'
//! `key == Rails.application.config.session_options[:key]` refuses as an
//! UNKNOWN-vs-String equality.
//!
//! The value behind `:key` is knowable at transpile time, so this grounds
//! the read to the typed accessor instead: `session_cookie_key` on
//! `Rails::Application`, defaulted in runtime/ruby/rails.rb and overridden
//! per app by the `config.session_store` lift in ingest. The result types
//! String on every target and needs no config bag at all.
//!
//! Scoped to `:key` deliberately. The other options are consumed by rack's
//! cookie-store middleware, which we don't run — grounding them would
//! imply a fidelity we don't have. They keep refusing, loudly, if an app
//! ever reads one.
//!
//! Note this is the APPLICATION's config, not the per-request bag:
//! `request.session_options[:skip] = true` is rack's writable
//! per-request options and is left alone (different receiver, and a
//! write, so the `[]` read shape below can't match it).

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_session_options_lowering(app: &mut App) {
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    for (_name, default) in &mut action.opt_params {
                        rewrite(default);
                    }
                    rewrite(&mut action.body);
                }
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => rewrite(expr),
                _ => {}
            }
        }
    }
    for class in &mut app.library_classes {
        for m in &mut class.methods {
            rewrite(&mut m.body);
        }
    }
}

/// Is `e` the literal `:key`?
fn is_key_symbol(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "key")
}

/// For `<base>.config.session_options`, the `<base>` receiver.
fn session_options_base(e: &Expr) -> Option<&Expr> {
    let ExprNode::Send { recv: Some(cfg), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "session_options" || !args.is_empty() {
        return None;
    }
    let ExprNode::Send { recv: Some(base), method, args, block: None, .. } = &*cfg.node else {
        return None;
    };
    if method.as_str() != "config" || !args.is_empty() {
        return None;
    }
    Some(base)
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    // Indexed reads are `Send "[]"` (the write side is `LValue::Index`,
    // which is why `request.session_options[:skip] = true` can't match).
    let base = match &*expr.node {
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "[]" && args.len() == 1 && is_key_symbol(&args[0]) =>
        {
            session_options_base(recv).cloned()
        }
        _ => None,
    };
    let Some(base) = base else { return };
    let span = expr.span;
    *expr = Expr::new(span, ExprNode::Send {
        recv: Some(base),
        method: Symbol::from("session_cookie_key"),
        args: Vec::new(),
        block: None,
        parenthesized: false,
    });
}
