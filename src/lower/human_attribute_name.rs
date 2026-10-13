//! `Model.human_attribute_name(:attr)` → the String Rails answers, from
//! the model's stamped i18n (`crate::i18n`): its locale's attribute name,
//! else the humanized attribute.
//!
//! Only a literal attribute on a receiver that names an Active Record
//! model folds, and only that shape is typed; a dynamic attribute keeps
//! analyze's error, since no runtime carries the locale table.
//! Hook bodies, test bodies and views all reach it (form headers and
//! table columns call it from templates).

use std::collections::HashMap;

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::i18n::ModelI18n;
use crate::ident::ClassId;
use crate::ty::Ty;

pub fn apply_human_attribute_name_lowering(app: &mut App) {
    let models: HashMap<ClassId, ModelI18n> = app
        .models
        .iter()
        .filter(|m| m.i18n.scope != "activemodel")
        .map(|m| (m.name.clone(), m.i18n.clone()))
        .collect();
    super::for_each_owned_hook_body(app, &mut |owner, body| rewrite(body, owner, &models));
    super::for_each_test_body(app, &mut |body| rewrite(body, None, &models));
    for view in &mut app.views {
        rewrite(&mut view.body, None, &models);
    }
}

/// The model a `human_attribute_name` receiver names: `Article`, or
/// `record.class` on a record of one.
pub(crate) fn receiver_model(recv: &Expr) -> Option<ClassId> {
    match &*recv.node {
        ExprNode::Const { path } => Some(ClassId(crate::ident::Symbol::from(
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        ))),
        ExprNode::Send { recv: Some(inner), method, args, .. }
            if method.as_str() == "class" && args.is_empty() =>
        {
            match inner.ty.as_ref() {
                Some(Ty::Class { id, .. }) => Some(id.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The attribute of a foldable `human_attribute_name` call.
pub(crate) fn literal_attribute(args: &[Expr]) -> Option<&str> {
    match args {
        [arg] => match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str()),
            ExprNode::Lit { value: Literal::Str { value } } => Some(value.as_str()),
            _ => None,
        },
        _ => None,
    }
}

fn rewrite(expr: &mut Expr, owner: Option<&ClassId>, models: &HashMap<ClassId, ModelI18n>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, owner, models));
    let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "human_attribute_name" {
        return;
    }
    // A bare call folds only where analyze typed it: on the class side, not an instance's NoMethodError.
    let model = match recv {
        Some(recv) => receiver_model(recv),
        None if expr.ty == Some(Ty::Str) => owner.cloned(),
        None => None,
    };
    let Some(i18n) = model.and_then(|id| models.get(&id)) else { return };
    let Some(attr) = literal_attribute(args) else { return };
    let value = i18n.human_attribute_name(attr);
    *expr.node = ExprNode::Lit { value: Literal::Str { value } };
    expr.ty = Some(Ty::Str);
}
