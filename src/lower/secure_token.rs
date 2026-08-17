//! `has_secure_token` — Rails' macro that fills a string column with a
//! generated unique token, synthesized in the shared model lowering
//! (all targets): a `before_create` default plus `regenerate_<attr>`.
//!
//! A DROPPED declaration here is not an absent method, it is a WRONG
//! COLUMN VALUE. campfire's `Session` declares `has_secure_token`, the
//! table has a UNIQUE index on `sessions.token`, and this runtime's
//! storage defaults a string slot to `""` — so with the macro
//! unclaimed the first session saved fine and the second died on
//! `UNIQUE constraint failed: sessions.token`, two test files' first
//! failure and a landmine under every future test that signs in twice.
//! That is the shape the unsupported-diagnostic ledger exists to
//! surface, and this is the entry coming off it.
//!
//! **Generator**: `SecureRandom.alphanumeric(<length>)` rather than
//! Rails' `SecureRandom.base58`, which is not core Ruby — it arrives
//! with `active_support/core_ext/securerandom`, and the emitted app
//! does not load ActiveSupport. `alphanumeric` is core, is already
//! carried by every target (campfire's own `Account::Joinable` and
//! `User::Bot` call it verbatim), and answers the same contract: a
//! random string of the declared length. The alphabet differs from
//! Rails'; the length does not.
//!
//! **Divergence, deliberate**: Rails 7.1+ defaults to `on:
//! :initialize`, so `Session.new.token` is already populated. Here the
//! token is filled in `before_create`, so it is readable from the
//! moment the record is SAVED. Every corpus call site reads the token
//! after a save (that is what a session token is for). `on: :create`
//! is exactly this lowering; `on: :initialize` gets it too rather than
//! silently keeping half of Rails' default, and the gap is recorded in
//! docs/pipeline/runtime.md.
//!
//! Shaped after `lower::secure_password` — the sibling `has_secure_*`
//! macro over the same kind of declaration.

use super::model_to_library::markers::fold_into_or_push;
use super::model_to_library::{fn_sig, push_synth_instance_method};
use crate::dialect::{AccessorKind, Model, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// Rails' `MINIMUM_TOKEN_LENGTH`, and its default.
const DEFAULT_LENGTH: i64 = 24;

/// One `has_secure_token` declaration this pass claims.
pub(crate) struct SecureTokenDecl {
    pub(crate) attr: Symbol,
    pub(crate) length: i64,
    pub(crate) span: Span,
}

/// The declarations in `body` this pass expands. The ONE place that
/// decides what is claimed — `report_unclaimed_unknowns` asks it by
/// span rather than re-deriving the shape, so a form that stops being
/// expanded starts warning again in the same commit.
pub(crate) fn secure_token_decls(body: &[ModelBodyItem]) -> Vec<SecureTokenDecl> {
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "has_secure_token" {
            continue;
        }
        let mut attr = Symbol::from("token");
        let mut length = DEFAULT_LENGTH;
        let mut ok = true;
        for (i, arg) in args.iter().enumerate() {
            match &*arg.node {
                ExprNode::Lit { value: Literal::Sym { value } } if i == 0 => {
                    attr = value.clone();
                }
                ExprNode::Hash { entries, .. } => {
                    for (k, v) in entries {
                        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                            ok = false;
                            continue;
                        };
                        match (key.as_str(), &*v.node) {
                            ("length", ExprNode::Lit { value: Literal::Int { value } }) => {
                                length = *value;
                            }
                            // Both spellings lower to `before_create`;
                            // see the divergence note in the header.
                            ("on", ExprNode::Lit { value: Literal::Sym { value } })
                                if matches!(value.as_str(), "create" | "initialize") => {}
                            // An option whose expansion would differ
                            // (`on: :update`, a computed length) stays
                            // unclaimed and keeps warning: half an
                            // expansion is worse than none.
                            _ => ok = false,
                        }
                    }
                }
                _ => ok = false,
            }
        }
        if ok {
            out.push(SecureTokenDecl { attr, length, span: expr.span });
        }
    }
    out
}

/// Synthesize each declaration's `before_create` default and
/// `regenerate_<attr>`.
pub(crate) fn push_secure_token_methods(
    methods: &mut Vec<crate::dialect::MethodDef>,
    model: &Model,
) {
    for decl in secure_token_decls(&model.body) {
        let span = decl.span;
        // `self.<attr> = SecureRandom.alphanumeric(<length>) if
        // self.<attr>.blank?` — `blank?`, not `nil?`, because this
        // runtime's storage defaults a string slot to `""` rather than
        // nil, so a nil test would never fire (the same reasoning
        // `markers::rewrite_column_or_assign` spells out for `||=`).
        let guarded = Expr::new(
            span,
            ExprNode::If {
                cond: send(span, Some(column_read(span, &decl.attr)), "blank?", vec![]),
                then_branch: assign_token(span, &decl),
                else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            },
        );
        // Fold rather than push: a model may declare its own
        // `before_create` (campfire's `Session` does, one line below
        // the macro), and the runtime Base calls ONE hook method.
        // Running before `push_callback_methods` puts the macro's
        // assignment ahead of the app's body — Rails' declaration
        // order for the usual "macro first, callback after" spelling.
        fold_into_or_push(methods, model, "before_create", guarded);

        // `regenerate_<attr>` — Rails defines it alongside the
        // callback. No corpus call site today; emitted anyway because
        // claiming the declaration silences its unsupported warning,
        // and a silenced warning plus a missing method is exactly the
        // silent gap this pass exists to end.
        let mut body_stmts = vec![assign_token(span, &decl)];
        body_stmts.push(send(span, None, "save!", vec![]));
        push_synth_instance_method(
            methods,
            model,
            Symbol::from(format!("regenerate_{}", decl.attr.as_str())),
            Vec::<Param>::new(),
            Expr::new(span, ExprNode::Seq { exprs: body_stmts }),
            Some(fn_sig(vec![], Ty::Class { id: model.name.clone(), args: vec![] })),
            AccessorKind::Method,
            true,
        );
    }
}

/// `self.<attr> = SecureRandom.alphanumeric(<length>)`
fn assign_token(span: Span, decl: &SecureTokenDecl) -> Expr {
    let generator = send(
        span,
        Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("SecureRandom")] })),
        "alphanumeric",
        vec![Expr::new(span, ExprNode::Lit { value: Literal::Int { value: decl.length } })],
    );
    send(
        span,
        Some(Expr::new(span, ExprNode::SelfRef)),
        &format!("{}=", decl.attr.as_str()),
        vec![generator],
    )
}

/// `self.<attr>`
fn column_read(span: Span, attr: &Symbol) -> Expr {
    send(span, Some(Expr::new(span, ExprNode::SelfRef)), attr.as_str(), vec![])
}

fn send(span: Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: false,
        },
    )
}
