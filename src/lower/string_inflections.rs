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

/// The same grounding over a body a later pass retyped; answers whether
/// any call was grounded, so the caller knows to type the body again.
/// The controller lowering runs it after the typed-params rewrite turns
/// `helper[:field]` (untyped) into `helper.field` (String): only then
/// does `helper[:field].humanize` have a String receiver.
pub(crate) fn ground_in_place(expr: &mut Expr) -> bool {
    fn walk(expr: &mut Expr, changed: &mut bool) {
        expr.node.for_each_child_mut(&mut |c| walk(c, changed));
        *changed |= rewrite_node(expr);
    }
    let mut changed = false;
    walk(expr, &mut changed);
    changed
}

/// Grounds one call; answers whether it did.
pub(crate) fn rewrite_node(expr: &mut Expr) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else {
        return false;
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
        return true;
    }
    let arity_ok = match name {
        "humanize" | "titleize" | "underscore" | "demodulize" | "singularize" => args.is_empty(),
        // ActiveSupport's `String#remove` takes one or more patterns; the
        // runtime helper takes one. Multi-arg calls stay dynamic.
        "remove" => args.len() == 1,
        _ => return false,
    };
    if !arity_ok || !is_string(r.ty.as_ref()) {
        return false;
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
    true
}
