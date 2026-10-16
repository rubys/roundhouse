//! Encoded here, not at ingest: the choice of encoder
//! (`json_render_encode`) needs the analyzed type of the value.

use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

use super::rewrites::json_render_encode;

pub(super) fn encode_inertia_values(body: &Expr) -> Expr {
    let mut out = body.clone();
    rewrite(&mut out);
    out
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut |c| rewrite(c));
    let ExprNode::Send { recv, method, args, .. } = &mut *e.node else { return };
    let encoded = match (method.as_str(), recv.as_ref(), args.len()) {
        ("prop", Some(r), 2) if is_page_reader(r) => "prop_json",
        ("inertia_errors", None, 1) => "inertia_errors_json",
        _ => return,
    };
    *method = Symbol::from(encoded);
    let value = args.pop().expect("arity checked");
    args.push(json_render_encode(&value));
}

fn is_page_reader(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: None, method, args, .. }
        if method.as_str() == "inertia_page" && args.is_empty())
}
