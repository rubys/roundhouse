//! Which `pluck` calls read materialized rows instead of SQL.
//!
//! The lowerer rewrites those calls to `map`, and the typer has to
//! give them the reader types before that rewrite. One predicate so
//! the two passes cannot disagree, and so the typer does not import
//! the lowerer to ask.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub(crate) fn reader_columns(
    recv: &Expr,
    method: &Symbol,
    args: &[Expr],
    block: Option<&Expr>,
    materialized: &std::collections::HashSet<Symbol>,
) -> Option<Vec<Symbol>> {
    if method.as_str() != "pluck" || block.is_some() || args.is_empty() {
        return None;
    }
    // Symbol arguments only. `pluck(:"table.col")` and the string form
    // name a column through SQL rather than through a reader, and
    // nothing answers those on a materialized row.
    let mut cols: Vec<Symbol> = Vec::new();
    for a in args {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node else { return None };
        if value.as_str().contains('.') {
            return None;
        }
        cols.push(value.clone());
    }
    // THE RECEIVER'S TYPE IS THE DISCRIMINATOR, and it has to be,
    // because the two association shapes are not syntactically
    // different. A plain `has_many` reader hydrates and answers an
    // Array (`Room#memberships`); a `has_many :through` answers an
    // `ActiveRecord::Relation` (`User#rooms`, a joins chain). Both are
    // spelled `owner.name`. A Relation ALREADY has `pluck`, and its
    // `pluck` is a single-column SELECT — rewriting that one would
    // replace a projection with a whole-row hydrate.
    //
    // UNKNOWN TYPES ARE REWRITTEN, and that is safe rather than
    // optimistic: `map` is Enumerable, so it answers on an Array and on
    // a Relation both. The worst an unknown costs is a Relation losing
    // its single-column SELECT — where the alternative, leaving it, is
    // `undefined method 'pluck' for an instance of Array` on every
    // Array-valued one. campfire's `room.memberships` is exactly this
    // case: the site lives in a concern, where `room` has no type.
    // `Ty::Array` LOOKS like the discriminator and is not. The analyzer
    // types a `has_many :through` reader `Array[Room]` too, where the
    // emitted reader is an `ActiveRecord::Relation` over a joins chain —
    // the type is its approximation of "a collection", and it conflates
    // exactly the two shapes this pass has to tell apart. Gating on it
    // rewrote a Relation's single-column SELECT into a whole-row
    // hydrate, which is the opposite of the fix.
    //
    // So the ASSOCIATION KIND decides, because that is what actually
    // determines which reader gets emitted. A stated Relation or model
    // Class still short-circuits — those are certain, and cheap to
    // check.
    if matches!(
        &recv.ty,
        Some(crate::ty::Ty::Relation { .. }) | Some(crate::ty::Ty::Class { .. })
    ) {
        return None;
    }
    // A LOCAL or parameter that may hold an Array — typed `Array[…]`, or
    // a union with an Array in it — is the other way a record Array
    // reaches `pluck`: campfire's `Rooms::Direct.find_for(users)` does
    // `users.pluck(:id)`, and its callers pass a Relation
    // (`DirectsController`) or an Array (`find_or_create_for([jz,
    // kevin])`, once-campfire-rust's model scenario). ActiveSupport's
    // `Enumerable#pluck` answers the Array; our Array has none. `map` is
    // right for both halves — only a stated Relation, returned above,
    // keeps its single-column SELECT.
    let var_array = matches!(&*recv.node, ExprNode::Var { .. })
        && match &recv.ty {
            Some(crate::ty::Ty::Array { .. }) => true,
            Some(crate::ty::Ty::Union { variants }) => {
                variants.iter().any(|v| matches!(v, crate::ty::Ty::Array { .. }))
            }
            _ => false,
        };
    if !var_array {
        let ExprNode::Send { method: reader, .. } = &*recv.node else { return None };
        if !materialized.contains(reader) {
            return None;
        }
    }
    // A Const root is `Model.where(…).pluck(:id)`, arel's inlined
    // SELECT. Belt and braces: a model constant does not answer an
    // association name, so this should not trigger — and if it ever
    // does, the SELECT wins.
    if const_rooted(recv) {
        return None;
    }
    Some(cols)
}

/// Does this chain bottom out at a model constant?
///
/// `Room.where(x).order(y)` does; `room.memberships` does not. Walks
/// only the RECEIVER spine — an argument that happens to name a class
/// is not what the chain is rooted on.
fn const_rooted(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Const { .. } => true,
        ExprNode::Send { recv: Some(r), .. } => const_rooted(r),
        _ => false,
    }
}
