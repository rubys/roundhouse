//! Stage 1 — `NEEDS_PARENS` decide-pass walker.
//!
//! Walks each method body top-down. At every parent → child step, if
//! the parent's emit will use the child in a position that demands a
//! Rust *primary* expression (method-call receiver, `as` cast LHS),
//! and the child's emit shape would be non-primary (`as` cast in the
//! child, binary op, etc.), stamp `bits::NEEDS_PARENS` on the child.
//!
//! Render reads the bit at `emit_send_recv` (and other primary-
//! demanding sites). Producer code in `expr/send/dispatch.rs` and
//! friends stops adding defensive `(…)` outer wraps — the bit drives
//! the wrap centrally.
//!
//! Today's first slice covers the warnings actually produced by
//! `cargo build` on the emitted real-blog crate:
//!
//! - `(recv_s.len() as i64).method(…)` — `size`/`length`/`count` on
//!   `Ty::Array | Ty::Str | Ty::Sym | Ty::Hash` recvs emits with an
//!   `as i64` cast. The cast is non-primary, so chained-recv usage
//!   needs wrapping; arg/let-RHS/return usage doesn't.
//!
//! That single shape closes the cast-related warnings on real-blog
//! once paired with the producer-side cleanup in
//! `expr/send/dispatch.rs`. Subsequent expansions add more
//! `is_non_primary_emit` arms and parent contexts as warnings
//! surface in larger fixtures.

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Pattern};
use crate::ty::Ty;

use super::bits::NEEDS_PARENS;

/// Walk every method body in every class and stamp `NEEDS_PARENS`
/// where parent → child context demands a primary form.
pub fn stamp(classes: &mut [LibraryClass]) {
    for class in classes {
        for m in class.methods.iter_mut() {
            walk_root(&mut m.body);
        }
    }
}

/// Walk a method body from its root. The root itself is in a
/// non-primary-demanding context (return / expression-statement /
/// block tail), so we never stamp the root — only its descendants
/// per the parent rule that introduced them.
fn walk_root(e: &mut Expr) {
    walk(e, false);
}

/// Recursive walker. `demands_primary` says whether the parent's
/// emit places `e` in a primary-demanding position. When true and
/// `e` would emit as non-primary, stamp the bit.
fn walk(e: &mut Expr, demands_primary: bool) {
    if demands_primary && is_non_primary_emit(e) {
        e.decisions |= NEEDS_PARENS;
    }
    walk_children(e);
}

/// Recurse into children with the appropriate `demands_primary`
/// flag per slot. Most slots are not-demanding (function args,
/// let-RHS, return, block tail, binary-op operands). Only the
/// specific demanding contexts forward `true`.
fn walk_children(e: &mut Expr) {
    match &mut *e.node {
        // Send: recv is in primary-demanding context (method call
        // syntax `recv.method(args)` requires recv to bind as a
        // primary). Args and block body are in expression context.
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv.as_mut() {
                walk(r, true);
            }
            for a in args.iter_mut() {
                walk(a, false);
            }
            if let Some(b) = block.as_mut() {
                walk(b, false);
            }
        }
        // Cast (`expr as T`) — LHS must be unary-or-tighter. Same
        // predicate covers the common warnings.
        ExprNode::Cast { value, .. } => walk(value, true),
        // Everything else: descend into structural children without
        // demanding primary. Exhaustive match so we don't silently
        // skip new variants — defaults to non-demanding.
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, false);
                walk(v, false);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk(el, false);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk(expr, false);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, false);
            walk(right, false);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, false);
            walk(then_branch, false);
            walk(else_branch, false);
        }
        ExprNode::While { cond, body, .. } => {
            walk(cond, false);
            walk(body, false);
        }
        ExprNode::Seq { exprs } => {
            for x in exprs {
                walk(x, false);
            }
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            walk_lvalue(target);
            walk(value, false);
        }
        ExprNode::MultiAssign { targets, value } => {
            for t in targets {
                walk_lvalue(t);
            }
            walk(value, false);
        }
        ExprNode::Lambda { body, .. } => walk(body, false),
        ExprNode::Return { value } => walk(value, false),
        ExprNode::Raise { value } => walk(value, false),
        ExprNode::Yield { args } => {
            for a in args {
                walk(a, false);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value.as_mut() {
                walk(v, false);
            }
        }
        ExprNode::Splat { value } => walk(value, false),
        ExprNode::Super { args } => {
            if let Some(arglist) = args.as_mut() {
                for a in arglist {
                    walk(a, false);
                }
            }
        }
        ExprNode::Case { scrutinee, arms } => {
            walk(scrutinee, false);
            for arm in arms {
                walk_pattern(&mut arm.pattern);
                if let Some(g) = arm.guard.as_mut() {
                    walk(g, false);
                }
                walk(&mut arm.body, false);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk(body, false);
            for r in rescues {
                walk(&mut r.body, false);
            }
            if let Some(eb) = else_branch.as_mut() {
                walk(eb, false);
            }
            if let Some(en) = ensure.as_mut() {
                walk(en, false);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            walk(expr, false);
            walk(fallback, false);
        }
        ExprNode::Let { value, body, .. } => {
            walk(value, false);
            walk(body, false);
        }
        ExprNode::Apply { fun, args, block } => {
            walk(fun, false);
            for a in args {
                walk(a, false);
            }
            if let Some(b) = block.as_mut() {
                walk(b, false);
            }
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin.as_mut() {
                walk(b, false);
            }
            if let Some(e) = end.as_mut() {
                walk(e, false);
            }
        }
    }
}

fn walk_lvalue(lv: &mut LValue) {
    match lv {
        LValue::Var { .. } | LValue::Ivar { .. } | LValue::Const { .. } => {}
        LValue::Attr { recv, .. } => walk(recv, true),
        LValue::Index { recv, index } => {
            walk(recv, true);
            walk(index, false);
        }
    }
}

fn walk_pattern(p: &mut Pattern) {
    match p {
        Pattern::Wildcard | Pattern::Bind { .. } | Pattern::Lit { .. } => {}
        Pattern::Array { elems, .. } => {
            for el in elems {
                walk_pattern(el);
            }
        }
        Pattern::Record { fields, .. } => {
            for (_, sub) in fields {
                walk_pattern(sub);
            }
        }
    }
}

/// Predicate: does this Expr's emit shape produce a Rust *non-primary*
/// expression? Today's first slice is conservative — only the cases
/// that produce visible `unused_parens` warnings on real-blog get
/// classified as non-primary. Everything else returns `false` and
/// no `NEEDS_PARENS` bit gets stamped (render emits the child bare).
///
/// Extending: add an arm when a new emit shape surfaces as a paren
/// warning in a real fixture, with a comment so reviewers can trace
/// which warning the arm closes.
fn is_non_primary_emit(e: &Expr) -> bool {
    match &*e.node {
        // `recv.size`/`length`/`count` on Array/Str/Sym/Hash recvs
        // dispatches in `expr/send/dispatch.rs` to `recv.len() as
        // i64`. The `as` cast is non-primary; chained-recv use needs
        // a wrap. Mirrors the dispatch table — keep in sync if those
        // arms change.
        ExprNode::Send { recv: Some(r), method, args, .. } if args.is_empty() => {
            let name = method.as_str();
            if !matches!(name, "size" | "length" | "count") {
                return false;
            }
            matches!(
                r.ty.as_ref().map(peel_nil),
                Some(Ty::Array { .. })
                    | Some(Ty::Str)
                    | Some(Ty::Sym)
                    | Some(Ty::Hash { .. })
            )
        }
        // `expr as T` — non-primary by definition. Real-blog doesn't
        // surface ExprNode::Cast in a recv position today (the cast
        // sites all live inside Send dispatch above), but covering
        // the IR shape keeps the predicate sound for fixtures we'll
        // add later.
        ExprNode::Cast { .. } => true,
        _ => false,
    }
}

/// `Union<T, Nil>` → `T`. Mirrors the peel used in
/// `dispatch_method_by_recv_ty` so we classify on the dispatch
/// view of the recv Ty rather than the body-typer's nilable shape.
fn peel_nil(ty: &Ty) -> &Ty {
    if let Ty::Union { variants } = ty {
        for v in variants {
            if !matches!(v, Ty::Nil) {
                return v;
            }
        }
    }
    ty
}
