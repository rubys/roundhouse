//! Encoded here, not at ingest: the choice of encoder
//! (`json_render_encode`) needs the analyzed type of the value.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

use super::rewrites::json_render_encode;

pub(super) fn encode_inertia_values(body: &Expr) -> Expr {
    let mut out = body.clone();
    rewrite(&mut out);
    out
}

fn rewrite(e: &mut Expr) {
    if let Some(replacement) = runtime_prop(e) {
        *e = replacement;
    }
    e.node.for_each_child_mut(&mut |c| rewrite(c));
    if let ExprNode::Send { recv: Some(page), method, args, .. } = &*e.node
        && is_page_reader(page) && method.as_str() == "lazy_prop"
        && let [key, value] = args.as_slice()
        && may_be_runtime_prop(value.ty.as_ref())
    {
        *e = apply_prop(e, page, key, value);
        return;
    }
    let ExprNode::Send { recv, method, args, .. } = &mut *e.node else { return };
    let encoded = match (method.as_str(), recv.as_ref(), args.len()) {
        ("prop" | "lazy_prop", Some(r), 2) if is_page_reader(r) => "prop_json",
        ("inertia_errors", None, 1) => "inertia_errors_json",
        _ => return,
    };
    *method = Symbol::from(encoded);
    let value = args.pop().expect("arity checked");
    args.push(json_render_encode(&value));
}

fn runtime_prop(e: &Expr) -> Option<Expr> {
    let ExprNode::If { cond, then_branch, else_branch } = &*e.node else { return None };
    if !matches!(&*else_branch.node, ExprNode::Lit { value: Literal::Nil }) {
        return None;
    }
    let ExprNode::Send { recv: Some(page), method, args: guard_args, .. } = &*cond.node else {
        return None;
    };
    if !is_page_reader(page) || method.as_str() != "eager?" {
        return None;
    }
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*then_branch.node else {
        return None;
    };
    let ([key], [prop_key, value]) = (guard_args.as_slice(), args.as_slice()) else {
        return None;
    };
    if !is_page_reader(recv) || method.as_str() != "prop" || key.node != prop_key.node
        || !may_be_runtime_prop(value.ty.as_ref())
    {
        return None;
    }
    Some(apply_prop(e, page, key, value))
}

fn apply_prop(e: &Expr, page: &Expr, key: &Expr, value: &Expr) -> Expr {
    Expr::new(e.span, ExprNode::Send {
        recv: None,
        method: Symbol::from("apply_inertia_prop"),
        args: vec![page.clone(), key.clone(), value.clone()],
        block: None,
        parenthesized: true,
    })
}

fn may_be_runtime_prop(ty: Option<&Ty>) -> bool {
    match ty {
        None | Some(Ty::Untyped | Ty::Var { .. } | Ty::Fn { .. } | Ty::SelfInstance) => true,
        Some(Ty::Class { id, .. }) => matches!(id.0.as_str(), "InertiaRails::Prop" | "Proc" | "Object" | "BasicObject"),
        Some(Ty::Union { variants }) => variants.iter().any(|v| may_be_runtime_prop(Some(v))),
        _ => false,
    }
}

fn is_page_reader(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: None, method, args, .. }
        if method.as_str() == "inertia_page" && args.is_empty())
}
