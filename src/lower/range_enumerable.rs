// Not each target's Range: an Integer range's Enumerable calls go through `to_a`, the Array every target already answers.
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

// Not `each_with_index` / `each_slice`: they answer their receiver, which `to_a` would turn into an Array.
pub(crate) const THROUGH_ARRAY: &[&str] = &[
    "map", "collect", "flat_map", "collect_concat", "filter_map", "select", "filter", "reject",
    "reduce", "inject", "each_with_object", "sort_by", "group_by", "partition", "min_by", "max_by",
    "find", "detect", "any?", "all?", "none?", "index_by", "index_with", "zip",
];

pub(crate) fn integer_range(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Class { id, args }) if id.0.as_str() == "Range" && matches!(args.as_slice(), [Ty::Int]))
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(r), method, .. } = &mut *expr.node else { return };
    if !THROUGH_ARRAY.contains(&method.as_str()) || !integer_range(r.ty.as_ref()) {
        return;
    }
    let span = r.span;
    let range = std::mem::replace(r, Expr::new(span, ExprNode::Lit { value: crate::expr::Literal::Nil }));
    let mut to_a = Expr::new(
        span,
        ExprNode::Send { recv: Some(range), method: Symbol::from("to_a"), args: vec![], block: None, parenthesized: false },
    );
    to_a.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
    *r = to_a;
}
