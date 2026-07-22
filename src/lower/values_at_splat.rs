//! `hash.values_at(*keys)` → `keys.map { |__k| hash[__k] }`.
//!
//! Exact Ruby semantics — `values_at` is defined as the per-key lookup
//! in order, `nil` for missing keys — with the splat gone. AOT targets
//! price a splat into a variadic builtin poorly (spinel compiled the
//! splatted array as ONE key and mis-typed the C; lobsters'
//! `comments_by_thread_id.values_at(*thread_ids).compact`), while the
//! block form is plain vocabulary every target speaks. Only the
//! splat-of-one-expression call rewrites, and only when the receiver
//! is a bare local/ivar read — the block re-reads it per element, so a
//! receiver with effects must keep its original shape. Literal-key
//! `values_at(a, b)` stays verbatim.

use crate::app::App;
use crate::expr::{BlockStyle, Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_values_at_splat_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else {
        return;
    };
    if method.as_str() != "values_at" || args.len() != 1 {
        return;
    }
    if !matches!(&*recv.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) {
        return;
    }
    let ExprNode::Splat { value: keys } = &*args[0].node else { return };

    let key_var = Symbol::from("__k");
    let lookup = Expr::new(
        e.span,
        ExprNode::Send {
            recv: Some(recv.clone()),
            method: Symbol::from("[]"),
            args: vec![Expr::new(e.span, ExprNode::Var {
                id: crate::ident::VarId(0),
                name: key_var.clone(),
            })],
            block: None,
            parenthesized: false,
        },
    );
    let block = Expr::new(
        e.span,
        ExprNode::Lambda {
            params: vec![key_var],
            block_param: None,
            body: lookup,
            block_style: BlockStyle::Brace,
        },
    );
    let span = e.span;
    *e = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(keys.clone()),
            method: Symbol::from("map"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    );
}
