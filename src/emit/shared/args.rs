//! Shared call-argument helpers.
//!
//! Decisions (not syntax) about how a call's argument list is shaped,
//! reused across the per-target emitters. Each target renders the split
//! result in its own syntax (Kotlin named args, C# named args, Elixir
//! keyword lists, …); only the structural decision lives here.

use crate::expr::{Expr, ExprNode};

/// Split off a trailing Ruby keyword-arguments hash.
///
/// Ingest flags a call's final `Hash` with `kwargs: true` when it came
/// from Ruby keyword-argument syntax (`foo(a, b, x: 1, y: 2)`) rather
/// than an explicit hash literal. When present, return the leading
/// positional args and the kwarg entries `(key, value)`; otherwise
/// return all args as positional with `None`.
///
/// The `kwargs: true` flag is the sole gate — a sym-keyed *map literal*
/// argument (`kwargs: false`) is left in the positional list, matching
/// every target's existing inline check.
pub fn split_trailing_kwargs(args: &[Expr]) -> (&[Expr], Option<&[(Expr, Expr)]>) {
    if let Some((last, head)) = args.split_last() {
        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
            return (head, Some(entries));
        }
    }
    (args, None)
}
