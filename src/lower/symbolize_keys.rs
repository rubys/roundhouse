//! ActiveSupport `Hash#symbolize_keys` (and `deep_dup`) grounding: on a receiver the
//! analyzer stamped `Hash[Symbol, _]`, the call is the IDENTITY, so it
//! becomes the receiver.
//!
//! Like `blank?` and `parameterize`, `symbolize_keys` is a core_ext
//! reopen only the CRuby overlay can host — every transpiled runtime
//! either cannot reopen a builtin or, on spinel AOT, has no method to
//! dispatch at all. campfire's `WebPush::Notification`:
//!
//! ```text
//! { subject: "mailto:…" }.merge Rails.configuration.x.vapid.symbolize_keys
//! ```
//!
//! The config read now answers a symbol-keyed Hash (the lifted group
//! reader, `ingest::app`), so the conversion has nothing to do — and
//! spinel was passing the unresolved call's poly result into
//! `sp_SymPolyHash_merge`'s `sp_SymPolyHash *`.
//!
//! ONLY WHEN THE KEYS ARE ALREADY SYMBOLS. A `Hash[String, _]` receiver
//! keeps its dynamic call: converting it is real work — a new hash with
//! every key interned — and inventing that here would be a rewrite
//! nobody has priced, not a grounding. The residue is honest: CRuby
//! serves it through the overlay, and a strict target reports it.
//!
//! Receiver effects are not a concern the way they are in
//! `lower::blank`: the receiver is used exactly once, in the same
//! position, so evaluation order and count are unchanged.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

#[allow(dead_code)]
pub fn apply_symbolize_keys_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    rewrite_node(expr);
}

/// The request's params (`params` / `@params`): a String-keyed Hash in
/// every emitted program. Only a reader typed as the request's params
/// (`ActionController::Parameters`), as a Hash, or left untyped
/// qualifies: one typed otherwise, such as a model's own `params`
/// String, is some other value.
fn is_request_params(e: &Expr) -> bool {
    let typed_as_params = match e.ty.as_ref() {
        None | Some(Ty::Untyped | Ty::Var { .. } | Ty::Hash { .. }) => true,
        Some(Ty::Class { id, .. }) => id.0.as_str() == "ActionController::Parameters",
        _ => false,
    };
    if !typed_as_params {
        return false;
    }
    match &*e.node {
        ExprNode::Send { recv: None, method, args, block: None, .. } => method.as_str() == "params" && args.is_empty(),
        ExprNode::Ivar { name } => name.as_str() == "params",
        _ => false,
    }
}

/// A value with no static type read out of a container: an index,
/// `fetch` or `dig` read, or the temporary `a&.b` binds it to
/// (`(t = h["order"]) && t.as_json`). Such a value is a Hash, an Array
/// or a scalar, whose `as_json` is Rails' core_ext one, and
/// `ActiveSupport::JSON.jsonable` answers it. Any other untyped
/// receiver (a plain local, an ivar, a helper call that the controller
/// lowering may still type as a params class) can be a record with its
/// own `as_json`, which `jsonable` does not model, so it is left alone.
fn is_open_value(e: &Expr) -> bool {
    if !matches!(e.ty.as_ref(), None | Some(Ty::Untyped | Ty::Var { .. })) {
        return false;
    }
    match &*e.node {
        ExprNode::Var { name, .. } => name.as_str().starts_with("__safe_nav"),
        ExprNode::Send { recv: Some(r), method, .. } => {
            !matches!(&*r.node, ExprNode::SelfRef)
                && matches!(method.as_str(), "[]" | "fetch" | "dig")
        }
        _ => false,
    }
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let replacement = match &mut *expr.node {
        // A typed, no-block Hash#deep_merge call is routed through the
        // shared helper because Spinel cannot dispatch this ActiveSupport
        // core extension on its native Hash representation.
        ExprNode::Send {
            recv: Some(receiver),
            method,
            args,
            block: None,
            ..
        } if method.as_str() == "deep_merge"
            && args.len() == 1
            && matches!(receiver.ty.as_ref(), Some(Ty::Hash { .. }))
            && matches!(args[0].ty.as_ref(), Some(Ty::Hash { .. })) =>
        {
            let mut call = Expr::new(
                expr.span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        expr.span,
                        ExprNode::Const {
                            path: vec![crate::ident::Symbol::from("ActiveSupport")],
                        },
                    )),
                    method: crate::ident::Symbol::from("deep_merge"),
                    args: vec![receiver.clone(), args[0].clone()],
                    block: None,
                    parenthesized: true,
                },
            );
            call.ty = expr.ty.clone();
            Some(call)
        }
        // ActiveSupport's `deep_dup` on a typed Hash or Array — the core_ext
        // reopen the CRuby overlay hosts, as the function every scaffold
        // tree ships (`ActiveSupport.deep_dup`, runtime/ruby/
        // active_support_ext.rb). campfire's `CachedResponses` snapshots
        // the session (`session.to_hash.deep_dup`) before a cached render.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "deep_dup"
                && args.is_empty()
                && matches!(r.ty.as_ref(), Some(Ty::Hash { .. } | Ty::Array { .. })) =>
        {
            let ty = r.ty.clone();
            let mut call = active_support_call(expr.span, "deep_dup", r.clone());
            call.ty = ty;
            Some(call)
        }
        // `as_json` on a typed Hash or Array (a request's params, a
        // literal built in the action): ActiveSupport's core_ext reopen,
        // which nothing hosts on Spinel ("undefined method 'as_json' for
        // an instance of Hash"). `ActiveSupport::JSON.jsonable` is that
        // conversion for the values such a container holds (String keys,
        // Symbols as Strings, nested Hashes and Arrays), and raises on
        // anything it does not model rather than answer other JSON.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "as_json"
                && args.is_empty()
                && (matches!(r.ty.as_ref(), Some(Ty::Hash { .. } | Ty::Array { .. }))
                    || is_request_params(r)
                    || is_open_value(r)) =>
        {
            let ty = match r.ty.as_ref() {
                Some(Ty::Array { .. }) => Ty::Array { elem: std::sync::Arc::new(Ty::Untyped) },
                Some(Ty::Hash { .. }) => Ty::Hash {
                    key: std::sync::Arc::new(Ty::Str),
                    value: std::sync::Arc::new(Ty::Untyped),
                },
                _ if is_request_params(r) => Ty::Hash {
                    key: std::sync::Arc::new(Ty::Str),
                    value: std::sync::Arc::new(Ty::Untyped),
                },
                _ => Ty::Untyped,
            };
            let mut call = Expr::new(
                expr.span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        expr.span,
                        ExprNode::Const {
                            path: vec![
                                crate::ident::Symbol::from("ActiveSupport"),
                                crate::ident::Symbol::from("JSON"),
                            ],
                        },
                    )),
                    method: crate::ident::Symbol::from("jsonable"),
                    args: vec![r.clone()],
                    block: None,
                    parenthesized: true,
                },
            );
            call.ty = Some(ty);
            Some(call)
        }
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "stringify_keys" && args.is_empty() =>
        {
            match r.ty.as_ref() {
                Some(Ty::Hash { key, .. }) if **key == Ty::Str => Some(r.clone()),
                Some(Ty::Hash { key, value }) if **key == Ty::Sym => {
                    let value = value.clone();
                    let mut call = active_support_call(expr.span, "stringify_keys", r.clone());
                    call.ty = Some(Ty::Hash { key: std::sync::Arc::new(Ty::Str), value });
                    Some(call)
                }
                _ => None,
            }
        }
        // Not every `deep_symbolize_keys`: only a receiver whose values cannot be hashes, where the deep form is the shallow one.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if matches!(method.as_str(), "symbolize_keys" | "symbolize_keys!" | "deep_symbolize_keys")
                && args.is_empty()
                && (method.as_str() != "deep_symbolize_keys"
                    || matches!(r.ty.as_ref(), Some(Ty::Hash { value, .. }) if is_scalar(value))) =>
        {
            // `Result#first` answers `Hash[String, _]?`; a nil there
            // raises on either spelling, so the nilable form counts.
            let hash_ty = match r.ty.as_ref() {
                Some(Ty::Union { variants }) => {
                    let non_nil: Vec<&Ty> = variants.iter().filter(|v| **v != Ty::Nil).collect();
                    if non_nil.len() == 1 { Some(non_nil[0]) } else { None }
                }
                other => other,
            };
            match hash_ty {
                Some(Ty::Hash { key, .. }) if **key == Ty::Sym && matches!(r.ty, Some(Ty::Hash { .. })) => {
                    Some(r.clone())
                }
                // A String-keyed receiver: the real conversion, through
                // `ActiveSupport.symbolize_keys` (runtime/ruby/
                // active_support_ext.rb) — a new hash, as Rails' non-bang
                // form answers. The bang form mutates its receiver in
                // place, which the new hash can stand in for only when
                // nothing reads the receiver again: a TEMPORARY
                // (`exec_query(sql).first.symbolize_keys!`). On a local
                // or ivar it stays put.
                Some(Ty::Hash { key, value }) if **key == Ty::Str => {
                    let bang = method.as_str().ends_with('!');
                    let named = matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. });
                    if bang && named {
                        None
                    } else {
                        let value = value.clone();
                        let mut call = Expr::new(
                            expr.span,
                            ExprNode::Send {
                                recv: Some(Expr::new(
                                    expr.span,
                                    ExprNode::Const { path: vec![crate::ident::Symbol::from("ActiveSupport")] },
                                )),
                                method: crate::ident::Symbol::from("symbolize_keys"),
                                args: vec![r.clone()],
                                block: None,
                                parenthesized: true,
                            },
                        );
                        call.ty = Some(Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value });
                        Some(call)
                    }
                }
                _ => None,
            }
        }
        // A scalar `Hash#to_query` is `ViewHelpers.to_query`. Nested
        // values stay on the ruby-family reopen, which sorts and
        // brackets them.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "to_query" && args.is_empty() && scalar_hash(r.ty.as_ref()) =>
        {
            let mut call = Expr::new(
                expr.span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        expr.span,
                        ExprNode::Const {
                            path: vec![
                                crate::ident::Symbol::from("ActionView"),
                                crate::ident::Symbol::from("ViewHelpers"),
                            ],
                        },
                    )),
                    method: crate::ident::Symbol::from("to_query"),
                    args: vec![r.clone()],
                    block: None,
                    parenthesized: true,
                },
            );
            call.ty = Some(Ty::Str);
            Some(call)
        }
        // Every other zero-arg `to_query`: a Hash whose values may nest,
        // or a receiver inference could not type
        // (`request.query_parameters.deep_dup.to_query`). The walk is
        // `ActiveSupport.to_query` (runtime/ruby/active_support_ext.rb),
        // Rails' own grammar and ordering, so the result is the same
        // bytes on every target. `to_query(key)` is the Object / Array /
        // nil form and keeps refusing.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "to_query"
                && args.is_empty()
                && matches!(r.ty.as_ref(), None | Some(Ty::Var { .. } | Ty::Untyped | Ty::Hash { .. })) =>
        {
            let mut call = active_support_call(expr.span, "to_query", r.clone());
            call.ty = Some(Ty::Str);
            Some(call)
        }
        _ => None,
    };
    if let Some(r) = replacement {
        *expr = r;
    }
}

fn scalar_hash(ty: Option<&Ty>) -> bool {
    match ty {
        Some(Ty::Hash { value, .. }) => is_scalar(value),
        Some(Ty::Union { variants }) => {
            let hashes: Vec<&Ty> = variants.iter().filter(|v| matches!(v, Ty::Hash { .. })).collect();
            hashes.len() == 1
                && scalar_hash(hashes.first().copied())
                && variants.iter().all(|v| matches!(v, Ty::Hash { .. } | Ty::Nil))
        }
        _ => false,
    }
}

fn is_scalar(ty: &Ty) -> bool {
    match ty {
        Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool | Ty::Nil | Ty::Time => true,
        // An empty literal is an open variable, not a nested hash. The
        // ruby emit has no `Hash#to_query` of its own, so leaving that
        // call ungrounded is a missing method. A value typed `Untyped`
        // can still be a Hash or an Array, and that stays on the
        // dynamic path.
        Ty::Var { .. } => true,
        Ty::Union { variants } => variants.iter().all(is_scalar),
        _ => false,
    }
}

fn active_support_call(span: crate::span::Span, method: &str, arg: Expr) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![crate::ident::Symbol::from("ActiveSupport")] },
            )),
            method: crate::ident::Symbol::from(method),
            args: vec![arg],
            block: None,
            parenthesized: true,
        },
    )
}

#[cfg(test)]
mod as_json_tests {
    use super::rewrite_node;
    use crate::expr::{Expr, ExprNode};
    use crate::ident::Symbol;
    use crate::span::Span;
    use crate::ty::Ty;

    fn as_json_on_params(params_ty: Option<Ty>) -> Expr {
        let mut params = Expr::new(
            Span::synthetic(),
            ExprNode::Send { recv: None, method: Symbol::from("params"), args: vec![], block: None, parenthesized: false },
        );
        params.ty = params_ty;
        let mut call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(params),
                method: Symbol::from("as_json"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        rewrite_node(&mut call);
        call
    }

    fn is_jsonable(e: &Expr) -> bool {
        matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "jsonable")
    }

    /// A `params` reader the analyzer typed as something other than a
    /// Hash (a model's own `params` String, say) is not the request's
    /// params: its `as_json` is left alone.
    #[test]
    fn a_params_reader_typed_as_a_string_is_not_the_request_params() {
        let call = as_json_on_params(Some(Ty::Str));
        assert!(!is_jsonable(&call), "{call:?}");
        assert!(call.ty.is_none(), "stamped {:?}", call.ty);
        let other = Ty::Class { id: crate::ident::ClassId(Symbol::from("Gadget")), args: vec![].into() };
        assert!(!is_jsonable(&as_json_on_params(Some(other))));
    }

    #[test]
    fn the_request_params_untyped_or_hash_are_grounded() {
        let parameters = Ty::Class {
            id: crate::ident::ClassId(Symbol::from("ActionController::Parameters")),
            args: vec![].into(),
        };
        for ty in [
            None,
            Some(Ty::Untyped),
            Some(Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(Ty::Untyped) }),
            Some(parameters),
        ] {
            assert!(is_jsonable(&as_json_on_params(ty.clone())), "{ty:?}");
        }
    }
}
