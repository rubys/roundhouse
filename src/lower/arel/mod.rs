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
    Order, Predicate, PreloadDirective, Select, Update, Value, ValueType,
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
    rewrite_arel_in_expr_with_assocs(expr, schema, registry, &[]);
}

/// As `rewrite_arel_in_expr`, but with the app's association graph so
/// `includes(:assoc)` chains lower to eager-load preloads (issue #27).
/// The 3-arg wrapper passes an empty graph → legacy drop-includes.
pub fn rewrite_arel_in_expr_with_assocs(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[crate::lower::model_associations::AssociationEdge],
) {
    // Names (ivars/locals) the body later refines with relation-chain
    // methods (`@moderations.where(...)` after `@moderations =
    // Moderation.all...`). Materializing the assigned chain here would
    // hand those refiners an Array — leave such statements on the
    // runtime Relation path.
    let mut refined = std::collections::HashSet::new();
    collect_relation_refined_names(expr, &mut refined);
    rewrite_arel_inner(expr, schema, registry, assocs, &refined);
}

const RELATION_REFINERS: &[&str] = &[
    "where", "not", "joins", "left_outer_joins", "left_joins", "order", "group", "having",
    "limit", "offset", "merge", "includes", "preload", "eager_load", "references", "distinct",
    "select", "where!", "order!", "reorder", "rewhere",
];

fn collect_relation_refined_names(
    expr: &Expr,
    out: &mut std::collections::HashSet<crate::ident::Symbol>,
) {
    if let ExprNode::Send { recv: Some(r), method, .. } = expr.node.as_ref() {
        if RELATION_REFINERS.contains(&method.as_str()) {
            match r.node.as_ref() {
                ExprNode::Ivar { name } | ExprNode::Var { name, .. } => {
                    out.insert(name.clone());
                }
                _ => {}
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_relation_refined_names(c, out));
}

fn rewrite_arel_inner(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[crate::lower::model_associations::AssociationEdge],
    refined: &std::collections::HashSet<crate::ident::Symbol>,
) {
    if let ExprNode::Assign { target, .. } = expr.node.as_ref() {
        let name = match target {
            crate::expr::LValue::Ivar { name } => Some(name),
            crate::expr::LValue::Var { name, .. } => Some(name),
            _ => None,
        };
        if name.is_some_and(|n| refined.contains(n)) {
            return;
        }
    }
    if let ExprNode::Send { .. } = expr.node.as_ref() {
        if let Some((op, owner)) =
            build::try_build_arel_with_assocs(expr, schema, registry, assocs)
        {
            let mut replacement = SqliteVisitor.visit(&op, schema, &owner);
            // The expansion replaces the recognized chain wholesale;
            // its provenance is the chain call site. Subtrees the
            // builder lifted out of the chain (predicate values, …)
            // keep their own, tighter spans.
            replacement.inherit_span(expr.span);
            *expr = replacement;
            return;
        }
    }
    // Inline sibling of the refined-names guard above: this Send is a
    // relation refiner whose chain did NOT lift (a lifted chain was
    // replaced wholesale and returned before reaching here — string
    // `order("tag asc")`, a chained `.where`, `references(...)`, …).
    // Recursing into its receiver would materialize the liftable base
    // underneath (`Category.all`, the has_many FK query) and strand the
    // refiner on a hydrated Array — `results.order("tag asc")`,
    // NoMethodError on every lane and a hard compile stop under AOT.
    // Leave the whole chain to the runtime Relation (the scope-chain
    // normalizer re-roots surviving `Const`-headed chains onto
    // `ActiveRecord::Relation.new(Model)` at emit). Spine Sends' args
    // and blocks are ordinary value positions and still rewrite. A
    // refiner WITH a block (`.select { … }`) is an Enumerable call on
    // materialized rows, not a chain link — the claim stays.
    let unlifted_refiner = matches!(
        expr.node.as_ref(),
        ExprNode::Send { recv: Some(_), block: None, method, .. }
            if RELATION_REFINERS.contains(&method.as_str())
    );
    if unlifted_refiner {
        let ExprNode::Send { recv: Some(recv), args, .. } = &mut *expr.node else {
            unreachable!("matched Send with recv above");
        };
        rewrite_arel_spine_args(recv, schema, registry, assocs, refined);
        for a in args {
            rewrite_arel_inner(a, schema, registry, assocs, refined);
        }
        return;
    }
    walk_subexprs_mut(expr, &mut |e| {
        rewrite_arel_inner(e, schema, registry, assocs, refined)
    });
    // Post-pass: when an Arel rewrite landed a multi-stmt hydrate Seq
    // in a *value* position — directly as an Assign value
    // (`@articles = <hydrate Seq>`) or nested inside a larger
    // expression (`@stories = period(<hydrate Seq>)`, where the
    // recognizer only matched the innermost `Story.includes(...)` and
    // left the Seq buried as a chain receiver) — hoist the Seq's
    // leading stmts out ahead of the enclosing statement and collapse
    // the Seq to its final expression. The Ruby emitter can't render an
    // inline multi-stmt value (`x = (a; b; c)`), so normalize
    // structurally.
    if let ExprNode::Seq { exprs } = &mut *expr.node {
        hoist_value_seqs_in_stmts(exprs);
    }
}

/// Rewrite value positions inside a chain-receiver spine WITHOUT
/// claiming the spine itself: every Send along the receiver path feeds
/// its value to an unlifted relation refiner, so materializing one
/// would strand the chain on a hydrated Array. The spine Sends' args
/// and blocks are ordinary value positions and rewrite normally.
/// Non-Send spine roots (a Const, an Ivar, an If picking a branch, …)
/// stay untouched for the same reason — anything materialized inside
/// them still becomes the chain's receiver value.
fn rewrite_arel_spine_args(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[crate::lower::model_associations::AssociationEdge],
    refined: &std::collections::HashSet<crate::ident::Symbol>,
) {
    if let ExprNode::Send { recv, args, block, .. } = &mut *expr.node {
        if let Some(r) = recv {
            rewrite_arel_spine_args(r, schema, registry, assocs, refined);
        }
        for a in args {
            rewrite_arel_inner(a, schema, registry, assocs, refined);
        }
        if let Some(b) = block {
            rewrite_arel_inner(b, schema, registry, assocs, refined);
        }
    }
}

/// For each statement in a Seq's stmt list, hoist any multi-stmt Seq an
/// Arel rewrite landed in one of its value positions (see
/// [`hoist_value_seqs`]), inserting the hoisted stmts ahead of it.
fn hoist_value_seqs_in_stmts(stmts: &mut Vec<Expr>) {
    let mut i = 0;
    while i < stmts.len() {
        let mut hoisted = Vec::new();
        hoist_value_seqs(&mut stmts[i], &mut hoisted);
        if hoisted.is_empty() {
            i += 1;
            continue;
        }
        let added = hoisted.len();
        for (j, stmt) in hoisted.into_iter().enumerate() {
            stmts.insert(i + j, stmt);
        }
        i += added + 1;
    }
}

/// Recurse through the *value* positions of `e` (call recv/args, assign
/// value, operands, array/hash values, …) and hoist every nested Seq
/// into `hoisted`, replacing it with its final expression. Statement-
/// context children (block bodies, if/while branches, nested stmt Seqs)
/// are NOT descended — their own enclosing Seq's post-pass handles them.
///
/// Note: two hydrate Seqs hoisted from one statement would both bind the
/// visitor's fixed `stmt`/`results` locals and collide; that multi-query-
/// per-statement case is a pre-existing visitor-naming limitation, not
/// introduced here (every recognized site uses the same var names).
fn hoist_value_seqs(e: &mut Expr, hoisted: &mut Vec<Expr>) {
    match &mut *e.node {
        ExprNode::Send { recv, args, .. } => {
            if let Some(r) = recv {
                hoist_value_child(r, hoisted);
            }
            for a in args {
                hoist_value_child(a, hoisted);
            }
        }
        ExprNode::Apply { fun, args, .. } => {
            hoist_value_child(fun, hoisted);
            for a in args {
                hoist_value_child(a, hoisted);
            }
        }
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
            hoist_value_child(value, hoisted);
        }
        ExprNode::BoolOp { left, right, .. } => {
            hoist_value_child(left, hoisted);
            hoist_value_child(right, hoisted);
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                hoist_value_child(el, hoisted);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (_, v) in entries {
                hoist_value_child(v, hoisted);
            }
        }
        ExprNode::Return { value } | ExprNode::Raise { value } | ExprNode::Splat { value } => {
            hoist_value_child(value, hoisted);
        }
        ExprNode::Yield { args } => {
            for a in args {
                hoist_value_child(a, hoisted);
            }
        }
        _ => {}
    }
}

/// Process one value-position child: recurse into its own value
/// positions, then — if the child is itself a Seq — move its leading
/// statements into `hoisted` and collapse it to its final expression.
fn hoist_value_child(child: &mut Expr, hoisted: &mut Vec<Expr>) {
    hoist_value_seqs(child, hoisted);
    if matches!(&*child.node, ExprNode::Seq { .. }) {
        let placeholder = Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Lit { value: crate::expr::Literal::Nil },
        );
        let seq = std::mem::replace(child, placeholder);
        if let ExprNode::Seq { exprs } = *seq.node {
            let mut exprs = exprs;
            if let Some(last) = exprs.pop() {
                hoisted.extend(exprs);
                *child = last;
            }
            // Empty Seq → keep the nil placeholder.
        }
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
        | ExprNode::Retry
        | ExprNode::Redo
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
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
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
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                f(v);
            }
        }
        ExprNode::Splat { value } => {
            f(value);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{LValue, Literal};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn var(n: &str) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(n) })
    }
    fn lit(s: &str) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s.into() } })
    }
    fn assign(name: &str, value: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from(name) },
                value,
            },
        )
    }
    fn seq_node(exprs: Vec<Expr>) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
    }
    fn call(method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: true,
            },
        )
    }

    /// A hydrate-shaped Seq whose final expression is the `results` var.
    fn hydrate_seq() -> Expr {
        seq_node(vec![
            assign("stmt", lit("prepare")),
            assign("results", lit("[]")),
            var("results"),
        ])
    }

    #[test]
    fn hoists_query_seq_nested_in_call_arg() {
        // `@x = period(<hydrate Seq>)` — the Seq is buried in the call
        // arg; its leading stmts must hoist out and the call bind to the
        // Seq's final expr (`period(results)`).
        let mut stmts = vec![assign("x", call("period", vec![hydrate_seq()]))];
        hoist_value_seqs_in_stmts(&mut stmts);

        assert_eq!(stmts.len(), 3, "two leading stmts hoisted ahead of the assign");
        let ExprNode::Assign { value, .. } = &*stmts[2].node else { panic!("expected assign") };
        let ExprNode::Send { args, .. } = &*value.node else { panic!("expected period(...)") };
        assert!(
            matches!(&*args[0].node, ExprNode::Var { .. }),
            "the Seq arg collapsed to its `results` var"
        );
    }

    #[test]
    fn direct_assign_seq_still_hoists_unchanged() {
        // The original `@x = <hydrate Seq>` case must behave identically.
        let mut stmts = vec![assign("x", hydrate_seq())];
        hoist_value_seqs_in_stmts(&mut stmts);
        assert_eq!(stmts.len(), 3);
        let ExprNode::Assign { value, .. } = &*stmts[2].node else { panic!() };
        assert!(matches!(&*value.node, ExprNode::Var { .. }), "binds to the results var");
    }
}
