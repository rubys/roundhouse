//! `type_previously_changed?(to: "Rooms::Open")` → `type_previously_
//! changed? && type == "Rooms::Open"`.
//!
//! Rails' Dirty predicates take optional `from:`/`to:` bounds:
//! "changed, AND the value it changed from/to was this". The
//! synthesized predicates are ZERO-ARITY, because a signature is a
//! promise every caller pays for — Rust and Go have no default
//! arguments, so an optional kwarg on `<col>_previously_changed?`
//! widens the shape for every model of every app to serve the handful
//! of sites that pass one. Exactly the argument
//! `lower::route_format_suffix` makes about `format:`.
//!
//! So the bound moves to the CALL SITE, where it is a literal and
//! costs nothing:
//!
//! ```text
//! x_previously_changed?(to: V)          -> x_previously_changed? && x == V
//! x_previously_changed?(from: U)        -> x_previously_changed? && x_previously_was == U
//! x_previously_changed?(from: U, to: V) -> both conjuncts
//! ```
//!
//! `x` and `x_previously_was` are the reader and the value-half the
//! same synthesis already emits, so this composes what is there rather
//! than adding surface.
//!
//! Scoped to the two spellings Rails documents as the same question,
//! and to literal bounds — a computed one would still be correct here,
//! but nothing writes it and an unrecognized shape is left for the
//! ledger rather than guessed at.

use crate::app::App;
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_dirty_predicate_kwargs(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body);
        }
    }
}

/// The column a Dirty predicate names, in either spelling.
fn column_of(method: &str) -> Option<&str> {
    method
        .strip_suffix("_previously_changed?")
        .or_else(|| method.strip_prefix("saved_change_to_").and_then(|m| m.strip_suffix('?')))
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else { return };
    if args.len() != 1 {
        return;
    }
    let Some(column) = column_of(method.as_str()) else { return };
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return };
    let bound = |name: &str| {
        entries.iter().find_map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name => Some(v),
            _ => None,
        })
    };
    // Every entry has to be one this understands — an unrecognized
    // option would otherwise be dropped silently.
    if !entries.iter().all(|(k, _)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if matches!(value.as_str(), "from" | "to"))
    }) {
        return;
    }
    let span = expr.span;
    let recv = recv.clone();
    let read = |reader: String| {
        Expr::new(
            span,
            ExprNode::Send {
                recv: recv.clone(),
                method: Symbol::from(reader),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        )
    };
    let eq = |lhs: Expr, rhs: &Expr| {
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(lhs),
                method: Symbol::from("=="),
                args: vec![rhs.clone()],
                block: None,
                parenthesized: false,
            },
        )
    };
    let mut out = Expr::new(
        span,
        ExprNode::Send {
            recv: recv.clone(),
            method: method.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let and = |left: Expr, right: Expr| {
        Expr::new(
            span,
            ExprNode::BoolOp {
                op: BoolOpKind::And,
                surface: BoolOpSurface::Symbol,
                left,
                right,
            },
        )
    };
    if let Some(from) = bound("from") {
        out = and(out, eq(read(format!("{column}_previously_was")), from));
    }
    if let Some(to) = bound("to") {
        out = and(out, eq(read(column.to_string()), to));
    }
    *expr = out;
}
