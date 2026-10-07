// Not left on the String: ActiveSupport reopens of String that no
// ruby-family runtime ships as instance methods. Grounded as
// `ActiveSupport.<name>(receiver, …)` against `active_support_ext.rb`.
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
    rewrite_node(expr);
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    let name = method.as_str();
    let arity_ok = match name {
        "humanize" | "titleize" | "underscore" | "demodulize" | "singularize" => args.is_empty(),
        // ActiveSupport's `String#remove` takes one or more patterns; the
        // runtime helper takes one. Multi-arg calls stay dynamic.
        "remove" => args.len() == 1,
        _ => return,
    };
    if !arity_ok || !is_string(r.ty.as_ref()) {
        return;
    }
    let text = r.clone();
    let method = method.clone();
    let mut grounded_args = vec![text];
    grounded_args.extend(args.iter().cloned());
    *expr.node = ExprNode::Send {
        recv: Some(Expr::new(
            expr.span,
            ExprNode::Const {
                path: vec![Symbol::from("ActiveSupport")],
            },
        )),
        method,
        args: grounded_args,
        block: None,
        parenthesized: true,
    };
    expr.ty = Some(Ty::Str);
}
