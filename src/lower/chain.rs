//! ActiveRecord query-chain flattening.
//!
//! Scaffold controllers chain AR methods (`Article.includes(:comments)
//! .order(created_at: :desc).limit(10)`). Each target emitter renders
//! this as its own `.all()` starting expression plus a sequence of
//! modifier applications (`sorted(...)` in Python, `.sort(...)` in
//! TS, `{ let mut v = …; v.sort_by(…); v }` in Rust).
//!
//! The WALK across the chain is target-neutral IR: follow
//! `Send.recv` through calls recognized as query-builder methods,
//! collect each (method, args) layer in natural application order.
//! The RENDER per modifier is target-specific — emitters keep their
//! own `apply_*_chain_modifier` functions.
//!
//! Scope: only collects layers for methods recognized by
//! `catalog::is_query_builder_method`. Stops at the chain head
//! (a Const receiver, or any non-query-builder Send).

use crate::expr::{Expr, ExprNode};

/// One layer of a flattened AR chain — the method name + the
/// Ruby-side args passed to it. `recv` is already consumed by the
/// walk; emitters compose modifiers onto a running target
/// expression starting from `Target.all()`.
#[derive(Debug)]
pub struct ChainModifier<'a> {
    pub method: &'a str,
    pub args: &'a [Expr],
}

/// Walk the chain bottom-up, returning modifier layers in their
/// natural application order (outermost call LAST). Stops at the
/// chain's head (a Const or any non-query-builder Send).
pub fn collect_chain_modifiers<'a>(
    method: &'a str,
    args: &'a [Expr],
    recv: Option<&'a Expr>,
) -> Vec<ChainModifier<'a>> {
    let mut out = Vec::new();
    if let Some(r) = recv {
        if let ExprNode::Send {
            recv: inner_recv,
            method: inner_method,
            args: inner_args,
            ..
        } = &*r.node
        {
            if crate::catalog::is_query_builder_method(inner_method.as_str()) {
                out.extend(collect_chain_modifiers(
                    inner_method.as_str(),
                    inner_args,
                    inner_recv.as_ref(),
                ));
            }
        }
    }
    out.push(ChainModifier { method, args });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprNode;
    use crate::ident::Symbol;
    use crate::span::Span;

    fn send_(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: false,
            },
        )
    }

    fn const_(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from(name)],
            },
        )
    }

    #[test]
    fn single_modifier_collects_one_layer() {
        // `Article.all` with `recv = Article` const — only "all" layer.
        let head = const_("Article");
        let layers = collect_chain_modifiers("all", &[], Some(&head));
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].method, "all");
    }

    #[test]
    fn two_modifiers_collect_in_order() {
        // `Article.includes(:comments).order(...)` — inner layer
        // ("includes") comes first, outer ("order") last.
        let head = const_("Article");
        let inner = send_(Some(head), "includes", vec![]);
        let layers = collect_chain_modifiers("order", &[], Some(&inner));
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].method, "includes");
        assert_eq!(layers[1].method, "order");
    }

    #[test]
    fn stops_at_non_query_builder_send() {
        // `foo.bar.order` where `foo.bar` isn't recognized — only
        // "order" collected, walk halts at the non-builder Send.
        let foo = send_(None, "foo", vec![]);
        let bar = send_(Some(foo), "bar", vec![]);
        let layers = collect_chain_modifiers("order", &[], Some(&bar));
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].method, "order");
    }
}
