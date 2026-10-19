//! Predicate cond rewrite. `blank?` / `present?` become
//! `(recv || "").empty?`: the nil guard from `lower::blank::or_empty_string`,
//! without `strip`. A template receiver is untyped, so the string arm's
//! `strip` would raise on an array, hash, or relation. `any?` /
//! `empty?` / `none?` stay collection `empty?`, with a nil-safe variant
//! for known-nullable locals. A record reference has no `empty?`, so
//! `blank?` / `present?` on one stay a nil test.

use crate::expr::{Expr, ExprNode};

use super::send;

/// Rewrite Rails-style emptiness predicates. Applied to the cond of
/// every template-level `if`:
///   `recv.present?`  →  `!((recv || "").empty?)`
///   `recv.blank?`    →  `(recv || "").empty?`
///   `recv.any?`      →  `!recv.empty?`
///   `recv.empty?` / `recv.none?`  →  `recv.empty?`
/// Recursive through `BoolOp` so `a.present? && b.any?` rewrites both
/// sides; other shapes pass through unchanged.
///
/// `blank?` / `present?` do not take a second nil-check, and they do
/// not `strip`. The `|| ""` already answers nil, and it is the same
/// guard the typed string arm wraps around `strip`. A view has no
/// receiver type: `strip` on `@posts` or `article.errors` raises.
/// Whitespace-only flash text stays the pre-existing view gap.
/// `any?` / `empty?` / `none?` still grow a nil-check when `recv` is a
/// known-nullable local (a view's extra_param with a `nil` default) or
/// a nilable-scalar reader (`nilable_reads`: typed_store attributes
/// with no default):
///   `notice.any?`    →  `!notice.nil? && !notice.empty?`
///   `notice.empty?`  →  `notice.nil? || notice.empty?`
/// Without that check a collection predicate NoMethodErrors when the
/// controller omits the kwarg.
///
/// When `recv` reads a record REFERENCE — a `belongs_to`/`has_one`
/// reader (`story.domain`) or a foreign-key column (`ms.domain_id`),
/// recognized via `refs` (the app's association-reader names) or the
/// `_id` suffix — the value is a record/Integer or nil, and Rails'
/// `present?`/`blank?` degrade to the nil test (`Object#blank?` is
/// `!self` when there's no `empty?`):
///   `story.domain.present?` → `!story.domain.nil?`
///   `ms.domain_id.blank?`   → `ms.domain_id.nil?`
/// `any?`/`empty?`/`none?` keep their collection forms — they're not
/// written against references in the corpus, and silently nil-testing
/// them would change real collection semantics.
pub(super) fn rewrite_predicates(
    cond: &Expr,
    nullable: &std::collections::HashSet<String>,
    refs: &std::collections::HashSet<String>,
    nilable_reads: &std::collections::HashSet<String>,
) -> Expr {
    let new_node = match &*cond.node {
        ExprNode::Send {
            recv: Some(r),
            method,
            args,
            block: None,
            ..
        } if args.is_empty() => {
            let rewritten_recv = rewrite_predicates(r, nullable, refs, nilable_reads);
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
                // An index read (`flash[f]`, `params[:x]`) yields nil on a
                // missing key. Collection predicates (`empty?` / `any?`)
                // need the nil-check; `blank?` / `present?` already carry
                // `|| ""` and do not read this flag.
                ExprNode::Send { recv: Some(_), method, .. }
                    if method.as_str() == "[]" => true,
                // A nilable-scalar reader through a record — a
                // typed_store attribute with no default reads nil when
                // unset, so a collection predicate on it needs the guard.
                ExprNode::Send { recv: Some(_), method, args, block: None, .. }
                    if args.is_empty() =>
                {
                    nilable_reads.contains(method.as_str())
                }
                _ => false,
            };
            // Record-reference read: an association-reader name or a
            // foreign-key column (`_id`). See the doc comment.
            let recv_is_reference = match &*rewritten_recv.node {
                ExprNode::Send { recv: Some(_), method, args, block: None, .. }
                    if args.is_empty() =>
                {
                    refs.contains(method.as_str()) || method.as_str().ends_with("_id")
                }
                ExprNode::Ivar { name } => {
                    refs.contains(name.as_str()) || name.as_str().ends_with("_id")
                }
                _ => false,
            };
            if recv_is_reference && matches!(method.as_str(), "present?" | "blank?") {
                let nil_call =
                    send(Some(rewritten_recv), "nil?", Vec::new(), None, false);
                return if method.as_str() == "present?" {
                    send(None, "!", vec![nil_call], None, false)
                } else {
                    nil_call
                };
            }
            match method.as_str() {
                // Nil guard in the IR, no `strip`: the receiver is untyped.
                "present?" | "blank?" => {
                    let span = rewritten_recv.span;
                    let guarded = crate::lower::blank::or_empty_string(
                        span,
                        rewritten_recv,
                        crate::ty::Ty::Untyped,
                    );
                    let empty_call = send(Some(guarded), "empty?", Vec::new(), None, false);
                    return if method.as_str() == "present?" {
                        send(None, "!", vec![empty_call], None, false)
                    } else {
                        empty_call
                    };
                }
                "any?" => {
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
                "empty?" | "none?" => {
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
            left: rewrite_predicates(left, nullable, refs, nilable_reads),
            right: rewrite_predicates(right, nullable, refs, nilable_reads),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(|r| rewrite_predicates(r, nullable, refs, nilable_reads)),
            method: method.clone(),
            args: args.iter().map(|a| rewrite_predicates(a, nullable, refs, nilable_reads)).collect(),
            block: block.as_ref().map(|b| rewrite_predicates(b, nullable, refs, nilable_reads)),
            parenthesized: *parenthesized,
        },
        // `list.filter(&:present?)` — ingest expands the symbol-to-proc
        // to `{ |x| x.present? }`, over elements the view knows nothing
        // about (lobsters' story classes: `[:story, klass, cond && "upvoted",
        // …].filter(&:present?)`, Symbols, Strings and `false`). The
        // `.empty?` rewrite above would be wrong for a Symbol or `false`,
        // so the predicate is the runtime helper, which answers every kind
        // as Rails does (runtime/ruby/active_support_ext.rb). Only the
        // whole-body `param.present?` / `param.blank?` shape; any other
        // block is left alone.
        ExprNode::Lambda { params, extra_params, body, .. }
            if params.len() == 1
                && extra_params.is_empty()
                && matches!(&*body.node,
                    ExprNode::Send { recv: Some(r), method, args, block: None, .. }
                        if args.is_empty()
                            && matches!(method.as_str(), "present?" | "blank?")
                            && matches!(&*r.node, ExprNode::Var { name, .. } if *name == params[0])) =>
        {
            let ExprNode::Send { recv: Some(r), method, .. } = &*body.node else { unreachable!() };
            let mut node = (*cond.node).clone();
            if let ExprNode::Lambda { body, .. } = &mut node {
                *body = Expr::new(
                    body.span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            body.span,
                            ExprNode::Const { path: vec![crate::ident::Symbol::from("ActiveSupport")] },
                        )),
                        method: method.clone(),
                        args: vec![r.clone()],
                        block: None,
                        parenthesized: true,
                    },
                );
            }
            node
        }
        other => other.clone(),
    };
    Expr::new(cond.span, new_node)
}
