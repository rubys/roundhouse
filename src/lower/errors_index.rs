//! `errors[:field]` grounding: `<recv>.errors[:field]` →
//! `<recv>.error_messages_for("field")`.
//!
//! The shared runtime's `errors` is a plain `Array[String]` of FULL
//! messages, and `error_details` holds the `ActiveRecord::ErrorDetail`
//! each was added with — [`super::errors_add`] and the `validates`
//! lowering push both. Rails' `errors[:field]` is the messages on one
//! attribute WITHOUT its humanized name (`[ "is not public" ]`, not
//! `[ "Url is not public" ]`), which `Base#error_messages_for` answers
//! from the details, under any `errors.format` and for `:base` alike.
//!
//! Left alone the site is not merely unsupported, it is silently wrong
//! on CRuby too: `Array#[]` with a Symbol raises `TypeError: no implicit
//! conversion of Symbol into Integer`, which is what campfire's
//! `Opengraph::LocationTest` reported for three tests.
//!
//! Purely shape-directed, like its two siblings: the receiver of `[]`
//! must itself be a zero-arg `errors` send, which is what makes the
//! site our accumulator rather than some other object's `[]`.
//!
//! `errors.to_hash` is the same projection for every attribute at once:
//! `ActiveSupport.errors_to_hash(<recv>.error_details)` groups the
//! details' messages by attribute, in the order Rails does.
//!
//! Views take the same rewrite: the scaffold's form partial reads
//! `<record>.errors[:field].none?` / `.any?` inside a `class:` array,
//! which renders through `class_names` at request time.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

/// Rewrite `errors[:field]` reads across every hook, test and view body.
/// Returns the residue ledger:
/// recognizable `errors[…]` sites left dynamic, with the reason.
pub fn apply_errors_index_lowering(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_owned_hook_body(app, &mut |_, body| {
        rewrite(body, &mut diags);
        ledger_messages(body, &mut diags);
    });
    super::for_each_test_body(app, &mut |body| {
        rewrite(body, &mut diags);
        ledger_messages(body, &mut diags);
    });
    for view in &mut app.views {
        rewrite(&mut view.body, &mut diags);
        ledger_messages(&mut view.body, &mut diags);
    }
    diags
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "errors_index",
        "errors[]",
        expr.span,
        reason,
        format!(
            "`errors[:field]` left as dynamic dispatch ({reason}) — the error \
             accumulator is an Array[String] whose `[]` takes an Integer; \
             ground by hand or extend the errors_index lowering"
        ),
    )
}

/// Is this expression a zero-arg, block-free `errors` send — i.e. our
/// accumulator? The receiver of `errors` itself is unconstrained, so
/// both `errors[:x]` (a model reading its own) and `record.errors[:x]`
/// (a test reading another object's) match.
fn is_errors_read(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { method, args, block: None, .. }
            if method.as_str() == "errors" && args.is_empty()
    )
}

fn is_messages_read(e: &Expr) -> bool {
    matches!(&*e.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "messages" && args.is_empty() && is_errors_read(r))
}

fn send(span: crate::span::Span, recv: Expr, method: &str, args: Vec<Expr>, ty: Ty) -> Expr {
    let mut e = Expr::new(
        span,
        ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block: None, parenthesized: false },
    );
    e.ty = Some(ty);
    e
}

// Not a Hash: the accumulator is the Array of full messages, so each `messages` question is asked of it directly.
fn rewrite_messages(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let span = expr.span;
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node else { return };
    if !is_messages_read(r) {
        return;
    }
    let ExprNode::Send { recv: Some(errors), .. } = &*r.node else { return };
    let accumulator = errors.clone();
    let replacement = match (method.as_str(), args.as_slice()) {
        ("[]", [_]) => {
            *r = accumulator;
            None
        }
        ("key?" | "has_key?" | "include?" | "member?", [field]) => {
            let field = field.clone();
            let mut index = send(span, accumulator, "[]", vec![field], Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
            rewrite_index(&mut index, diags);
            Some(send(span, index, "any?", vec![], Ty::Bool))
        }
        ("blank?" | "empty?", []) => Some(send(span, accumulator, "empty?", vec![], Ty::Bool)),
        ("present?" | "any?", []) => Some(send(span, accumulator, "any?", vec![], Ty::Bool)),
        _ => None,
    };
    if let Some(new) = replacement {
        *expr = new;
    }
}

fn rewrite(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, diags));
    rewrite_messages(expr, diags);
    rewrite_index(expr, diags);
    rewrite_to_hash(expr);
}

/// `<owner>.errors.to_hash` → `ActiveSupport.errors_to_hash(<owner>.error_details)`, typed.
fn rewrite_to_hash(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(reader), method, args, block: None, .. } = &*expr.node else { return };
    if method.as_str() != "to_hash" || !args.is_empty() || !is_errors_read(reader) {
        return;
    }
    let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!("an errors reader") };
    let span = expr.span;
    let mut details = Expr::new(
        span,
        ExprNode::Send { recv: owner.clone(), method: Symbol::from("error_details"), args: Vec::new(), block: None, parenthesized: false },
    );
    details.ty = Some(super::errors_add::error_details_ty());
    let mut support = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] });
    support.ty = Some(Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveSupport")), args: vec![].into() });
    let mut call = Expr::new(
        span,
        ExprNode::Send { recv: Some(support), method: Symbol::from("errors_to_hash"), args: vec![details], block: None, parenthesized: true },
    );
    call.ty = Some(Ty::Hash {
        key: std::sync::Arc::new(Ty::Sym),
        value: std::sync::Arc::new(Ty::Array {
            elem: std::sync::Arc::new(Ty::Str),
        }),
    });
    *expr = call;
}

fn ledger_messages(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    if is_messages_read(expr) {
        diags.push(crate::lower::residue_diagnostic(
            "errors_index",
            "errors.messages",
            expr.span,
            "whole_hash",
            "`errors.messages` grounds only as `[]`, `key?`, `blank?`/`empty?` or `present?`/`any?`; the whole \
             Hash is not built"
                .to_string(),
        ));
    }
    expr.node.for_each_child_mut(&mut |c| ledger_messages(c, diags));
}

fn rewrite_index(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let matches_shape = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "[]" && args.len() == 1 && is_errors_read(r)
    );
    if !matches_shape {
        return;
    }
    let field = match &*expr.node {
        ExprNode::Send { args, .. } => match &*args[0].node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
            _ => None,
        },
        _ => unreachable!(),
    };
    let Some(field) = field else {
        diags.push(residue(expr, "dynamic field"));
        return;
    };
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, .. } = node else { unreachable!() };
    let ExprNode::Send { recv: owner, .. } = *recv.expect("checked Some above").node else { unreachable!() };
    *expr = messages_on(owner, field.as_str(), span);
}

/// `<owner>.error_messages_for("<field>")`, typed.
pub(crate) fn messages_on(owner: Option<Expr>, field: &str, span: crate::span::Span) -> Expr {
    let mut lit = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: field.to_string() } });
    lit.ty = Some(Ty::Str);
    let mut e = Expr::new(
        span,
        ExprNode::Send { recv: owner, method: Symbol::from("error_messages_for"), args: vec![lit], block: None, parenthesized: true },
    );
    // Stamp the result: callers run *after* analyze, and the post-lowering
    // `diagnose` walk reads stamped types without re-dispatching.
    e.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
    e
}

/// `<owner>.error_messages_for("<field>").any?`, typed.
pub(crate) fn has_errors_on(owner: Option<Expr>, field: &str, span: crate::span::Span) -> Expr {
    send(span, messages_on(owner, field, span), "any?", vec![], Ty::Bool)
}
