//! Predicate cond rewrite: collapse Rails-style emptiness predicates
//! into the `.empty?`-based forms spinel's runtime expects, with a
//! nil-safe variant for known-nullable locals.

use crate::expr::{Expr, ExprNode};

use super::send;

/// Rewrite Rails-style emptiness predicates to spinel-shape boolean
/// forms. Applied to the cond of every template-level `if`:
///   `recv.present?` / `recv.any?`  →  `!recv.empty?`
///   `recv.blank?`   / `recv.empty?` / `recv.none?`  →  `recv.empty?`
/// Recursive through `BoolOp` so `a.present? && b.any?` rewrites both
/// sides; other shapes pass through unchanged.
///
/// When `recv` is a known-nullable local (a view's extra_param with a
/// `nil` default), the nil-safe form is generated instead:
///   `notice.present?`  →  `!notice.nil? && !notice.empty?`
///   `notice.empty?`    →  `notice.nil? || notice.empty?`
/// matching spinel-blog's hand-written guards. Without the nil check
/// the body NoMethodErrors when the controller omits the flash kwarg.
pub(super) fn rewrite_predicates(cond: &Expr, nullable: &std::collections::HashSet<String>) -> Expr {
    let new_node = match &*cond.node {
        ExprNode::Send {
            recv: Some(r),
            method,
            args,
            block: None,
            ..
        } if args.is_empty() => {
            let rewritten_recv = rewrite_predicates(r, nullable);
            // Bareword references (Rails flash helpers like `notice`) are
            // parsed as `Send { recv: None, method: <name>, args: [] }`
            // until something binds them as Vars; accept either shape.
            let recv_is_nullable = match &*rewritten_recv.node {
                ExprNode::Var { name, .. } => nullable.contains(name.as_str()),
                ExprNode::Send {
                    recv: None,
                    method,
                    args,
                    block: None,
                    ..
                } if args.is_empty() => nullable.contains(method.as_str()),
                _ => false,
            };
            match method.as_str() {
                "present?" | "any?" => {
                    let empty_call = send(
                        Some(rewritten_recv.clone()),
                        "empty?",
                        Vec::new(),
                        None,
                        false,
                    );
                    let not_empty = send(None, "!", vec![empty_call], None, false);
                    if recv_is_nullable {
                        let nil_call = send(
                            Some(rewritten_recv),
                            "nil?",
                            Vec::new(),
                            None,
                            false,
                        );
                        let not_nil = send(None, "!", vec![nil_call], None, false);
                        return Expr::new(
                            cond.span,
                            ExprNode::BoolOp {
                                op: crate::expr::BoolOpKind::And,
                                surface: crate::expr::BoolOpSurface::Symbol,
                                left: not_nil,
                                right: not_empty,
                            },
                        );
                    }
                    return not_empty;
                }
                "blank?" | "empty?" | "none?" => {
                    let empty_call = send(
                        Some(rewritten_recv.clone()),
                        "empty?",
                        Vec::new(),
                        None,
                        false,
                    );
                    if recv_is_nullable {
                        let nil_call = send(
                            Some(rewritten_recv),
                            "nil?",
                            Vec::new(),
                            None,
                            false,
                        );
                        return Expr::new(
                            cond.span,
                            ExprNode::BoolOp {
                                op: crate::expr::BoolOpKind::Or,
                                surface: crate::expr::BoolOpSurface::Symbol,
                                left: nil_call,
                                right: empty_call,
                            },
                        );
                    }
                    return empty_call;
                }
                _ => ExprNode::Send {
                    recv: Some(rewritten_recv),
                    method: method.clone(),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            }
        }
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_predicates(left, nullable),
            right: rewrite_predicates(right, nullable),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(|r| rewrite_predicates(r, nullable)),
            method: method.clone(),
            args: args.iter().map(|a| rewrite_predicates(a, nullable)).collect(),
            block: block.as_ref().map(|b| rewrite_predicates(b, nullable)),
            parenthesized: *parenthesized,
        },
        other => other.clone(),
    };
    Expr::new(cond.span, new_node)
}
