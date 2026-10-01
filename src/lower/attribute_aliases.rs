// Not left as a call: `read_attribute`/`write_attribute` are Rails' spellings of a model's `[]`/`[]=`, and no runtime defines them by those names.
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_attribute_alias_lowering(app: &mut App) {
    let models: std::collections::HashSet<String> = app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    for model in &mut app.models {
        for item in &mut model.body {
            use crate::dialect::{Association, ModelBodyItem};
            match item {
                ModelBodyItem::Method { method, .. } => rewrite(&mut method.body, &models, true),
                ModelBodyItem::Scope { scope, .. } => rewrite(&mut scope.body, &models, true),
                ModelBodyItem::Callback { callback, .. } => {
                    if let Some(cond) = &mut callback.condition {
                        rewrite(cond, &models, true);
                    }
                }
                ModelBodyItem::Unknown { expr, .. } => rewrite(expr, &models, true),
                ModelBodyItem::Association { assoc: Association::HasMany { extension, .. }, .. } => {
                    for m in extension.iter_mut() {
                        rewrite(&mut m.body, &models, true);
                    }
                }
                _ => {}
            }
        }
    }
    let mut outside = |e: &mut Expr| rewrite(e, &models, false);
    super::for_each_hook_body(app, &mut outside);
    super::for_each_test_body(app, &mut outside);
    for view in &mut app.views {
        rewrite(&mut view.body, &models, false);
    }
}

fn rewrite(expr: &mut Expr, models: &std::collections::HashSet<String>, in_model: bool) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, models, in_model));
    let ExprNode::Send { recv, method, args, block: None, .. } = &mut *expr.node else { return };
    let to = match (method.as_str(), args.len()) {
        ("read_attribute", 1) => "[]",
        ("write_attribute", 2) => "[]=",
        _ => return,
    };
    let on_model = match recv {
        None => in_model,
        // Not only a typed model: a test body's locals reach this pass untyped, and these names are ActiveRecord's alone.
        Some(r) => match r.ty.as_ref() {
            None => true,
            Some(Ty::Class { id, .. }) => models.contains(id.0.as_str()),
            Some(_) => false,
        },
    };
    if !on_model {
        return;
    }
    if let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node {
        let name = Symbol::from(value.as_str());
        *args[0].node = ExprNode::Lit { value: Literal::Sym { value: name } };
        args[0].ty = Some(Ty::Sym);
    }
    if recv.is_none() {
        *recv = Some(Expr::new(expr.span, ExprNode::SelfRef));
    }
    *method = Symbol::from(to);
}
