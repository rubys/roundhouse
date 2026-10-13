//! `errors[:field]` grounding: `<recv>.errors[:field]` →
//! `ActiveSupport.errors_for(<recv>.errors, "Field ")`.
//!
//! The shared runtime's error accumulator is a plain `Array[String]` of
//! FULL messages — [`super::errors_add`] owns that invariant and bakes
//! `"#{humanized_field} #{message}"` at lower time, and
//! [`super::errors_full_messages`] folds Rails' `full_messages` hop
//! away because on our shape it is the identity. Rails' `errors[:field]`
//! is the other projection of the same data: the messages for one
//! attribute, WITHOUT the humanized prefix (`[ "is not public" ]`, not
//! `[ "Url is not public" ]`).
//!
//! Left alone the site is not merely unsupported, it is silently wrong
//! on CRuby too: `Array#[]` with a Symbol raises `TypeError: no implicit
//! conversion of Symbol into Integer`, which is what campfire's
//! `Opengraph::LocationTest` reported for three tests. So the pass
//! rewrites to a helper that re-derives the projection from the baked
//! prefix — the same "one shared `runtime/ruby` body prices every
//! target" placement [`super::presence_in`] documents, in the same
//! `ActiveSupport` module.
//!
//! WHAT THE PREFIX RE-DERIVATION CANNOT SEE. The accumulator keeps no
//! attribute column, so the helper matches on text. If one attribute's
//! humanized name is a PREFIX of another's — `url` and `url_host`
//! humanize to `"Url"` and `"Url host"` — then `errors[:url]` also
//! matches `"Url host can't be blank"` and answers `"host can't be
//! blank"`. That is a real divergence from Rails and it is recorded in
//! `docs/pipeline/runtime.md`; the fix is an attribute column on the
//! accumulator, which changes `@errors`' type in every strict target
//! and is not worth it until a corpus site needs it.
//!
//! `:base` DECLINES. Rails attaches `:base` messages to the record
//! rather than a field, and `errors_add` accordingly bakes them with NO
//! prefix — there is no text for the helper to match on, and every
//! prefixed message would be a false negative. Those sites join the
//! residue ledger instead.
//!
//! Purely shape-directed, like its two siblings: the receiver of `[]`
//! must itself be a zero-arg `errors` send, which is what makes the
//! site our accumulator rather than some other object's `[]`.
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
    let data = super::errors_add::LabelData::of(app);
    super::for_each_owned_hook_body(app, &mut |owner, body| {
        rewrite(body, &data.at(owner), &mut diags);
        ledger_messages(body, &mut diags);
    });
    super::for_each_test_body(app, &mut |body| {
        rewrite(body, &data.at(None), &mut diags);
        ledger_messages(body, &mut diags);
    });
    for view in &mut app.views {
        rewrite(&mut view.body, &data.at(None), &mut diags);
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
fn rewrite_messages(expr: &mut Expr, labels: &super::errors_add::Labels, diags: &mut Vec<Diagnostic>) {
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
            let mut index = send(span, accumulator, "[]", vec![field], Ty::Array { elem: Box::new(Ty::Str) });
            rewrite_index(&mut index, labels, diags);
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

fn rewrite(expr: &mut Expr, labels: &super::errors_add::Labels, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, labels, diags));
    rewrite_messages(expr, labels, diags);
    rewrite_index(expr, labels, diags);
}

fn ledger_messages(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    if is_messages_read(expr) {
        diags.push(crate::lower::residue_diagnostic(
            "errors_index",
            "errors.messages",
            expr.span,
            "whole_hash",
            "`errors.messages` grounds only as `[]`, `key?`, `blank?`/`empty?` or `present?`/`any?`; the error \
             accumulator keeps no attribute column to build the Hash from"
                .to_string(),
        ));
    }
    expr.node.for_each_child_mut(&mut |c| ledger_messages(c, diags));
}

fn rewrite_index(expr: &mut Expr, labels: &super::errors_add::Labels, diags: &mut Vec<Diagnostic>) {
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
    let ExprNode::Send { recv: Some(reader), .. } = &*expr.node else { unreachable!() };
    let owner_once = matches!(&*reader.node, ExprNode::Send { recv: owner, .. } if super::errors_add::owner_reads_once(owner.as_ref()));
    if !owner_once {
        diags.push(residue(expr, "the record is not a plain read, and picking by attribute reads it twice"));
        return;
    }
    let Some(prefix_text) = labels.message_prefix(reader, field.as_str()) else {
        diags.push(residue(expr, "errors.format does not end in the message"));
        return;
    };
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, .. } = node else { unreachable!() };
    *expr = errors_for_call(recv.expect("checked Some above"), field.as_str(), prefix_text, span);
}

/// `ActiveSupport.errors_for(<owner>.errors, <owner>.error_attributes, "<attribute>", "<prefix>")`, typed.
pub(crate) fn errors_for_call(accumulator: Expr, attribute: &str, prefix_text: String, span: crate::span::Span) -> Expr {
    let str_lit = |value: String| Expr::new(span, ExprNode::Lit { value: Literal::Str { value } });
    let ExprNode::Send { recv: owner, .. } = &*accumulator.node else { unreachable!("an errors reader") };
    let mut attributes = Expr::new(
        span,
        ExprNode::Send {
            recv: owner.clone(),
            method: Symbol::from("error_attributes"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    attributes.ty = Some(Ty::Array { elem: Box::new(Ty::Str) });
    let mut support = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] });
    support.ty = Some(Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveSupport")), args: vec![] });
    let mut call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(support),
            method: Symbol::from("errors_for"),
            args: vec![accumulator, attributes, str_lit(attribute.to_string()), str_lit(prefix_text)],
            block: None,
            parenthesized: true,
        },
    );
    // Stamp the result: callers run *after* analyze, and the
    // post-lowering `diagnose` walk reads stamped types without
    // re-dispatching, so an unstamped synthesis false-positives a
    // `send_dispatch_failed`.
    call.ty = Some(Ty::Array { elem: Box::new(Ty::Str) });
    call
}
