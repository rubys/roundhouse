//! `rel.order(Arel.sql("…"))` / `rel.reorder(Arel.sql("…"))` →
//! `rel.order_sql("…")` / `rel.reorder_sql("…")`.
//!
//! Rails' `order` refuses anything that is not a column name unless the
//! caller marks it as SQL with `Arel.sql`, which returns an
//! `Arel::Nodes::SqlLiteral`. A one-argument call is rewritten here,
//! before that node exists, so the fragment reaches `order_sql` as the
//! text the caller wrote — campfire's
//! `reorder(Arel.sql("+messages.created_at"))`, SQLite's unary plus that
//! keeps the planner off an index. A bare String `order` keeps its
//! column check. A call with binds is not this rewrite. `Arel.sql`
//! substitutes them, and `order_term` keeps an `Arel::Nodes::Node`.
//!
//! Only a call ON A RELATION (a chain, or the implicit receiver of a
//! scope body) is renamed: a model constant's `order` is the class
//! side, which has no `order_sql` to land on.

use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send { recv, method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    let renamed = match method.as_str() {
        "order" => "order_sql",
        "reorder" => "reorder_sql",
        _ => return,
    };
    if recv.as_ref().is_some_and(|r| matches!(&*r.node, ExprNode::Const { .. })) {
        return;
    }
    let [arg] = args.as_mut_slice() else { return };
    let ExprNode::Send { recv: Some(arel), method: sql, args: inner, block: None, .. } = &mut *arg.node
    else {
        return;
    };
    let is_arel = matches!(&*arel.node, ExprNode::Const { path }
        if path.len() == 1 && path[0].as_str() == "Arel");
    if !is_arel || sql.as_str() != "sql" || inner.len() != 1 {
        return;
    }
    let fragment = inner.remove(0);
    *arg = fragment;
    *method = Symbol::from(renamed);
}
