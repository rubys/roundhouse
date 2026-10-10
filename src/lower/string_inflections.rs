// Ground ActiveSupport extensions as static module calls: String helpers
// route through `ActiveSupport`, and Integer#ordinalize through
// `ActiveSupport::Inflector`. The shared implementation is in
// `active_support_inflections.rb`, loaded by the Ruby/Spinel scaffold.
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
    if name == "ordinalize" && args.is_empty() && matches!(r.ty.as_ref(), Some(Ty::Int)) {
        let number = r.clone();
        *expr.node = ExprNode::Send {
            recv: Some(Expr::new(
                expr.span,
                ExprNode::Const {
                    path: vec![Symbol::from("ActiveSupport"), Symbol::from("Inflector")],
                },
            )),
            method: Symbol::from("ordinalize"),
            args: vec![number],
            block: None,
            parenthesized: true,
        };
        expr.ty = Some(Ty::Str);
        return;
    }
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
