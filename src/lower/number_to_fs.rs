// Not `to_fs` left on the number: it is an ActiveSupport reopen of Numeric that no ruby-family runtime ships.
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_number_to_fs_grounding(app: &mut App) {
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
    if !is_delimited_to_fs(method, args) || !matches!(r.ty, Some(Ty::Int) | Some(Ty::Float)) {
        return;
    }
    let number = r.clone();
    *expr.node = ExprNode::Send {
        recv: Some(Expr::new(
            expr.span,
            ExprNode::Const {
                path: vec![Symbol::from("ActiveSupport")],
            },
        )),
        method: Symbol::from("number_delimited"),
        args: vec![number],
        block: None,
        parenthesized: true,
    };
    expr.ty = Some(Ty::Str);
}

pub(crate) fn is_delimited_to_fs(method: &Symbol, args: &[Expr]) -> bool {
    matches!(method.as_str(), "to_fs" | "to_formatted_s")
        && matches!(args, [a] if matches!(&*a.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "delimited"))
}
