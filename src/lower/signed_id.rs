//! Rails' signed ids, rewritten at the call site so the model name and
//! the purpose become one compile-time string:
//!
//!   signed_id(purpose: :avatar)
//!     -> ActiveRecord::SignedId.generate(self.id, "user/avatar", 0)
//!   signed_id(purpose: :transfer, expires_in: D)
//!     -> ActiveRecord::SignedId.generate(self.id, "user/transfer", D.to_i)
//!   find_signed(tok, purpose: :transfer)
//!     -> User.find_by(id: ActiveRecord::SignedId.verified_id(tok, "user/transfer"))
//!   find_signed!(tok, purpose: :avatar)
//!     -> User.find(ActiveRecord::SignedId.verified_id(tok, "user/avatar"))
//!
//! The wire work is already done — action_controller/message_verifier.rb
//! ships the envelope, the PBKDF2 key and both digests, and its header
//! comment named `avatar_token` as the caller it was built for. What was
//! missing is only the part a shared runtime cannot supply: Rails'
//! `combine_signed_id_purposes` reads `self.class.name`, and this
//! runtime is deliberately reflection-free (base.rb: "zero
//! metaprogramming"). At the call site the model is known, so the
//! purpose is a literal and no model needs to be able to name itself.
//!
//! Scope: a bare or `self` receiver in a MODEL body — including
//! concerns and association extensions, via `for_each_model_body_named`,
//! which is where campfire writes both (`User::Avatar`,
//! `User::Transferable`). An explicit receiver is left alone: `signed_id`
//! on somebody else's record would need THAT model's name, which the
//! walk does not carry.
//!
//! `find_signed` inside a `class_methods do` block also appears hoisted
//! onto the model itself; both copies are model bodies and both get
//! rewritten, which is why the rewrite must be idempotent-safe (it
//! matches only the unrewritten shape).

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::naming::underscore;
use crate::span::Span;

pub fn apply_signed_id_lowering(app: &mut App) {
    super::for_each_model_body_named(app, &mut |model, body| rewrite(model, body));
}

fn rewrite(model: &str, e: &mut Expr) {
    if let Some(replacement) = rewritten(model, e) {
        *e.node = *replacement.node;
        e.ty = None;
    }
    e.node.for_each_child_mut(&mut |c| rewrite(model, c));
}

fn rewritten(model: &str, e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else { return None };
    match recv {
        None => {}
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => {}
        Some(_) => return None,
    }
    let span = e.span;
    match method.as_str() {
        "signed_id" => {
            let [kwargs] = &args[..] else { return None };
            let opts = kwarg_entries(kwargs)?;
            let purpose = combined_purpose(model, sym_opt(&opts, "purpose")?);
            // Seconds, and `to_i` is what turns the corpus' spelling —
            // an `ActiveSupport::Duration` constant — into them. On an
            // Integer literal it is the identity, so both spellings of
            // `expires_in:` reach the runtime the same way.
            let expires = match opt(&opts, "expires_in") {
                Some(d) => send(span, Some(d.clone()), "to_i", vec![]),
                None => Expr::new(span, ExprNode::Lit { value: Literal::Int { value: 0 } }),
            };
            // Rails also takes `expires_at:` — an absolute instant, not
            // a duration. Nothing in the corpus writes it, so a call
            // carrying any option this rewrite does not reproduce is
            // left alone: it fails by name rather than silently minting
            // a token that never expires.
            if !only_options(opts, &["purpose", "expires_in"]) {
                return None;
            }
            Some(send(
                span,
                Some(const_path(span, &["ActiveRecord", "SignedId"])),
                "generate",
                vec![send(span, Some(Expr::new(span, ExprNode::SelfRef)), "id", vec![]), str_lit(span, &purpose), expires],
            ))
        }
        // Rails answers nil for a token that does not verify and raises
        // for the bang form; `find_by(id:)` and `find` reproduce that
        // pair over the sentinel `verified_id` returns.
        "find_signed" | "find_signed!" => {
            let [token, kwargs] = &args[..] else { return None };
            let opts = kwarg_entries(kwargs)?;
            if opts.len() != 1 {
                return None;
            }
            let purpose = combined_purpose(model, sym_opt(&opts, "purpose")?);
            // The BANG form verifies through the raising twin. Rails
            // separates the two failures — a token that does not verify
            // is `InvalidSignature`, a token that DOES and names no row
            // is `RecordNotFound` — and the sentinel cannot: `find(0)`
            // reports both as "Couldn't find … with id=0". The name is
            // what a rescue matches, so answering the wrong one means a
            // `rescue_from` that never fires.
            let verifier = if method.as_str() == "find_signed!" {
                "verified_id!"
            } else {
                "verified_id"
            };
            let id = send(
                span,
                Some(const_path(span, &["ActiveRecord", "SignedId"])),
                verifier,
                vec![token.clone(), str_lit(span, &purpose)],
            );
            let recv = const_path(span, &[model]);
            Some(if method.as_str() == "find_signed" {
                send(
                    span,
                    Some(recv),
                    "find_by",
                    vec![Expr::new(
                        span,
                        ExprNode::Hash {
                            entries: vec![(
                                Expr::new(
                                    span,
                                    ExprNode::Lit { value: Literal::Sym { value: Symbol::from("id") } },
                                ),
                                id,
                            )],
                            kwargs: true,
                        },
                    )],
                )
            } else {
                send(span, Some(recv), "find", vec![id])
            })
        }
        _ => None,
    }
}

/// Rails' `combine_signed_id_purposes`: the underscored model name,
/// then the caller's purpose. A namespaced model underscores to a path
/// (`Rooms::Open` → `rooms/open`), which is what Rails' `name.underscore`
/// gives too.
fn combined_purpose(model: &str, purpose: &str) -> String {
    format!("{}/{}", underscore(model), purpose)
}

fn only_options(entries: &[(Expr, Expr)], allowed: &[&str]) -> bool {
    entries.iter().all(|(k, _)| match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => allowed.contains(&value.as_str()),
        _ => false,
    })
}

fn kwarg_entries(e: &Expr) -> Option<&Vec<(Expr, Expr)>> {
    match &*e.node {
        ExprNode::Hash { entries, .. } => Some(entries),
        _ => None,
    }
}

fn opt<'a>(entries: &'a [(Expr, Expr)], key: &str) -> Option<&'a Expr> {
    entries.iter().find_map(|(k, v)| match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == key => Some(v),
        _ => None,
    })
}

fn sym_opt<'a>(entries: &'a [(Expr, Expr)], key: &str) -> Option<&'a str> {
    match &*opt(entries, key)?.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.as_str()),
        _ => None,
    }
}

fn send(span: Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn const_path(span: Span, parts: &[&str]) -> Expr {
    Expr::new(
        span,
        ExprNode::Const { path: parts.iter().map(|p| Symbol::from(*p)).collect() },
    )
}

fn str_lit(span: Span, value: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } },
        )
    }

    fn kwargs(entries: Vec<(&str, Expr)>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: entries.into_iter().map(|(k, v)| (sym(k), v)).collect(),
                kwargs: true,
            },
        )
    }

    fn call(method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: false,
            },
        )
    }

    fn rendered(e: &Expr) -> String {
        crate::emit::ruby::expr::emit_expr(e)
    }

    #[test]
    fn folds_the_model_name_into_the_purpose() {
        let mut e = call("signed_id", vec![kwargs(vec![("purpose", sym("avatar"))])]);
        rewrite("User", &mut e);
        assert_eq!(rendered(&e), "ActiveRecord::SignedId.generate(self.id, \"user/avatar\", 0)");
    }

    /// A namespaced model underscores to a PATH, which is what Rails'
    /// `name.underscore` gives — `Rooms::Open` → `rooms/open`, not
    /// `rooms::open`.
    #[test]
    fn a_namespaced_model_underscores_to_a_path() {
        let mut e = call("signed_id", vec![kwargs(vec![("purpose", sym("invite"))])]);
        rewrite("Rooms::Open", &mut e);
        assert!(rendered(&e).contains("\"rooms/open/invite\""), "{}", rendered(&e));
    }

    /// `expires_in:` is a Duration in the corpus; `to_i` is what makes
    /// it the seconds the runtime takes.
    #[test]
    fn expires_in_reaches_the_runtime_as_seconds() {
        let duration = Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("TRANSFER_LINK_EXPIRY_DURATION")] },
        );
        let mut e = call(
            "signed_id",
            vec![kwargs(vec![("purpose", sym("transfer")), ("expires_in", duration)])],
        );
        rewrite("User", &mut e);
        assert_eq!(
            rendered(&e),
            "ActiveRecord::SignedId.generate(self.id, \"user/transfer\", TRANSFER_LINK_EXPIRY_DURATION.to_i)"
        );
    }

    /// Rails' `expires_at:` takes an absolute instant this rewrite does
    /// not reproduce. Leaving the call alone keeps the NoMethodError,
    /// which beats minting a token that silently never expires.
    #[test]
    fn declines_an_option_it_does_not_reproduce() {
        let mut e = call(
            "signed_id",
            vec![kwargs(vec![("purpose", sym("transfer")), ("expires_at", sym("whenever"))])],
        );
        rewrite("User", &mut e);
        assert_eq!(rendered(&e), "signed_id purpose: :transfer, expires_at: :whenever");
    }

    /// The bang form raises on a miss and the plain one answers nil —
    /// `find` and `find_by` are that pair over the sentinel id. The
    /// VERIFIER differs too: the bang form uses the raising twin, so a
    /// token that does not verify is an `InvalidSignature` rather than a
    /// `RecordNotFound` for id 0. The name is what a `rescue_from`
    /// matches.
    #[test]
    fn find_signed_pairs_with_find_by_and_the_bang_with_find() {
        let token = || {
            Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("sid") },
            )
        };
        let mut plain =
            call("find_signed", vec![token(), kwargs(vec![("purpose", sym("transfer"))])]);
        rewrite("User", &mut plain);
        assert_eq!(
            rendered(&plain),
            "User.find_by(id: ActiveRecord::SignedId.verified_id(sid, \"user/transfer\"))"
        );

        let mut bang =
            call("find_signed!", vec![token(), kwargs(vec![("purpose", sym("avatar"))])]);
        rewrite("User", &mut bang);
        assert_eq!(
            rendered(&bang),
            "User.find(ActiveRecord::SignedId.verified_id!(sid, \"user/avatar\"))"
        );
    }

    /// `other.signed_id(...)` would need THAT model's name, which the
    /// model-body walk does not carry.
    #[test]
    fn leaves_an_explicit_receiver_alone() {
        let mut e = call("signed_id", vec![kwargs(vec![("purpose", sym("avatar"))])]);
        if let ExprNode::Send { recv, .. } = &mut *e.node {
            *recv = Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("other") },
            ));
        }
        rewrite("User", &mut e);
        assert!(matches!(&*e.node, ExprNode::Send { .. }));
        assert!(rendered(&e).starts_with("other.signed_id"), "{}", rendered(&e));
    }
}
