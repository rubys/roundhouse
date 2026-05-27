//! Arel IR — query algebra evaluable at transpile time.
//!
//! See `project_arel_compile_time_first.md` for the architectural
//! direction this implements. Phase 1 contains:
//!
//! - `ir`       — type definitions for the algebra (`ArelOp`,
//!                `Predicate`, `Value`, …).
//! - `visitor`  — `ArelVisitor` trait + `SqliteVisitor`
//!                implementation that turns an `ArelOp` into the
//!                same kind of `Expr` today's per-shape adapter
//!                methods produce.
//! - `build`    — `try_build_arel`: pattern recognizer that maps
//!                a Send call site to an `ArelOp`. Returns None
//!                for shapes the lowerer can't statically resolve;
//!                those route to runtime fallback in Phase 2.

pub mod build;
pub mod ir;
pub mod visitor;

pub use build::try_build_arel;
pub use ir::{
    ArelOp, Assignment, ColRef, ColumnSpec, Delete, Direction, Insert, Join, JoinKind, LimitSpec,
    Order, Predicate, Select, Update, Value, ValueType,
};
pub use visitor::{ArelVisitor, SqliteVisitor};

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::expr::{Expr, ExprNode, InterpPart};
use crate::ident::ClassId;
use crate::schema::Schema;

/// Rewrite an Expr tree in-place: every Send that `try_build_arel`
/// recognizes is replaced by the visitor-emitted Expr. Sends that
/// don't match are left intact; recursion continues into their
/// receiver, args, and block.
///
/// The replacement happens top-down: when an outer Send matches, we
/// don't recurse into its parts (they're consumed into the
/// visitor-built tree). Inner Sends inside the visitor's output
/// don't need re-inspection — they're target-runtime Db.* calls,
/// not user code.
pub fn rewrite_arel_in_expr(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
) {
    if let ExprNode::Send { .. } = expr.node.as_ref() {
        if let Some((op, owner)) = try_build_arel(expr, schema, registry) {
            let replacement = SqliteVisitor.visit(&op, schema, &owner);
            *expr = replacement;
            return;
        }
    }
    walk_subexprs_mut(expr, &mut |e| rewrite_arel_in_expr(e, schema, registry));
    // Post-pass: when an Arel rewrite landed a Seq in an Assign's
    // value slot (e.g. `@articles = <multi-row hydrate Seq>`),
    // hoist the Seq's leading stmts out into the enclosing Seq so
    // the assignment binds to the Seq's final expression rather
    // than chaining to the first stmt. Ruby `x = a; b; c` parses
    // as `x = a` then `b` then `c` — the parens-grouped form
    // `x = (a; b; c)` would also work but isn't what the Ruby
    // emitter produces, so normalize structurally instead.
    if let ExprNode::Seq { exprs } = &mut *expr.node {
        hoist_seq_assigns(exprs);
    }
}

/// Within a Seq's stmt list, replace any `Assign { target, value:
/// Seq { inner_exprs } }` with the inner Seq's leading stmts
/// followed by `Assign { target, value: <inner Seq's last expr> }`.
/// Generic normalization — applies to any rewrite that lifts a
/// multi-stmt expression into a value position.
fn hoist_seq_assigns(stmts: &mut Vec<Expr>) {
    let mut i = 0;
    while i < stmts.len() {
        let take_inner = matches!(
            &*stmts[i].node,
            ExprNode::Assign { value, .. } if matches!(&*value.node, ExprNode::Seq { .. })
        );
        if !take_inner {
            i += 1;
            continue;
        }
        // Decompose the Assign + inner Seq, then splice.
        let assign = std::mem::replace(
            &mut stmts[i],
            Expr::new(crate::span::Span::synthetic(), ExprNode::Lit { value: crate::expr::Literal::Nil }),
        );
        let (target, inner_exprs) = match *assign.node {
            ExprNode::Assign { target, value } => match *value.node {
                ExprNode::Seq { exprs } => (target, exprs),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        let mut leading = inner_exprs;
        let last = leading.pop().expect("Seq with at least one expr");
        let new_assign = Expr::new(
            assign.span,
            ExprNode::Assign { target, value: last },
        );
        // Replace the placeholder + insert leading stmts before it.
        stmts.remove(i);
        let added = leading.len();
        for (j, stmt) in leading.into_iter().enumerate() {
            stmts.insert(i + j, stmt);
        }
        stmts.insert(i + added, new_assign);
        i += added + 1;
    }
}

/// Mutable visitor for every direct sub-Expr of `expr`. Caller
/// applies whatever transform via `f`; this only handles the
/// recursion shape so adding a new ExprNode variant updates one
/// place.
fn walk_subexprs_mut(expr: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match &mut *expr.node {
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                f(k);
                f(v);
            }
        }
        ExprNode::Array { elements, .. } => {
            for e in elements {
                f(e);
            }
        }
        ExprNode::StringInterp { parts } => {
            for part in parts {
                if let InterpPart::Expr { expr } = part {
                    f(expr);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            f(left);
            f(right);
        }
        ExprNode::Let { value, body, .. } => {
            f(value);
            f(body);
        }
        ExprNode::Lambda { body, .. } => f(body),
        ExprNode::Apply { fun, args, block } => {
            f(fun);
            for a in args {
                f(a);
            }
            if let Some(b) = block {
                f(b);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                f(r);
            }
            for a in args {
                f(a);
            }
            if let Some(b) = block {
                f(b);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprNode::Case { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    f(g);
                }
                f(&mut arm.body);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                f(e);
            }
        }
        ExprNode::Assign { target, value } => {
            walk_lvalue_mut(target, f);
            f(value);
        }
        ExprNode::Yield { args } => {
            for a in args {
                f(a);
            }
        }
        ExprNode::Raise { value } => f(value),
        ExprNode::RescueModifier { expr, fallback } => {
            f(expr);
            f(fallback);
        }
        ExprNode::Return { value } => f(value),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    f(a);
                }
            }
        }
        ExprNode::Next { value } => {
            if let Some(v) = value {
                f(v);
            }
        }
        ExprNode::MultiAssign { targets, value } => {
            for t in targets {
                walk_lvalue_mut(t, f);
            }
            f(value);
        }
        ExprNode::While { cond, body, .. } => {
            f(cond);
            f(body);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                f(b);
            }
            if let Some(e) = end {
                f(e);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            f(body);
            for r in rescues {
                for c in &mut r.classes {
                    f(c);
                }
                f(&mut r.body);
            }
            if let Some(e) = else_branch {
                f(e);
            }
            if let Some(e) = ensure {
                f(e);
            }
        }
        ExprNode::Cast { value, .. } => f(value),
    }
}

fn walk_lvalue_mut(lv: &mut crate::expr::LValue, f: &mut dyn FnMut(&mut Expr)) {
    use crate::expr::LValue;
    match lv {
        LValue::Var { .. } | LValue::Ivar { .. } | LValue::Const { .. } => {}
        LValue::Attr { recv, .. } => f(recv),
        LValue::Index { recv, index } => {
            f(recv);
            f(index);
        }
    }
}
