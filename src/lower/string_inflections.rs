// Not left on the String: `humanize`/`titleize` are ActiveSupport reopens of String that no ruby-family runtime ships.
use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_string_inflection_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn is_string(ty: Option<&Ty>) -> bool {
    match ty {
        Some(Ty::Str) => true,
        Some(Ty::Union { variants }) => {
            variants.iter().any(|v| matches!(v, Ty::Str)) && variants.iter().all(|v| matches!(v, Ty::Str | Ty::Nil))
        }
        _ => false,
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else { return };
    if !matches!(method.as_str(), "humanize" | "titleize") || !args.is_empty() || !is_string(r.ty.as_ref()) {
        return;
    }
    let text = r.clone();
    let method = method.clone();
    *expr.node = ExprNode::Send {
        recv: Some(Expr::new(expr.span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })),
        method,
        args: vec![text],
        block: None,
        parenthesized: true,
    };
    expr.ty = Some(Ty::Str);
}
