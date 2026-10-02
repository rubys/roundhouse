// Not an `ActiveModel::Type::Boolean` instance: no ruby-family runtime ships the class, so the cast grounds to `ActiveSupport.cast_boolean`.
use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_boolean_cast_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send {
        recv: Some(r),
        method,
        args,
        block: None,
        ..
    } = &*expr.node
    else {
        return;
    };
    if method.as_str() != "cast" || args.len() != 1 || !is_boolean_type_new(r) {
        return;
    }
    let value = args[0].clone();
    *expr.node = ExprNode::Send {
        recv: Some(Expr::new(
            expr.span,
            ExprNode::Const {
                path: vec![Symbol::from("ActiveSupport")],
            },
        )),
        method: Symbol::from("cast_boolean"),
        args: vec![value],
        block: None,
        parenthesized: true,
    };
    expr.ty = Some(Ty::Union {
        variants: vec![Ty::Bool, Ty::Nil],
    });
}

fn is_boolean_type_new(e: &Expr) -> bool {
    matches!(&*e.node,
        ExprNode::Send { recv: Some(c), method, args, block: None, .. }
            if method.as_str() == "new" && args.is_empty()
                && matches!(&*c.node, ExprNode::Const { path }
                    if path.iter().map(|s| s.as_str()).collect::<Vec<_>>() == ["ActiveModel", "Type", "Boolean"]))
}
