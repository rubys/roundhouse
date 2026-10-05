//! Normalize CollectionProxy's `new` alias after analysis has typed the read.
//!
//! A method named like an association can return an unrelated class. The
//! reader must have the declared collection's analyzed type before `.new`
//! becomes `.build`.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::dialect::{Association, MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

type Associations = HashMap<ClassId, HashMap<Symbol, ClassId>>;

pub fn apply_association_new_lowering(app: &mut App) {
    let assocs: Associations = app
        .models
        .iter()
        .map(|model| {
            let mut overrides: HashSet<Symbol> = HashSet::new();
            for item in &model.body {
                match item {
                    ModelBodyItem::Method { method, .. }
                        if method.receiver == MethodReceiver::Instance =>
                    {
                        overrides.insert(method.name.clone());
                    }
                    ModelBodyItem::Unknown { expr, .. } => {
                        if let ExprNode::Send {
                            recv: None,
                            method,
                            args,
                            ..
                        } = &*expr.node
                        {
                            if matches!(
                                method.as_str(),
                                "attr_reader" | "attr_accessor" | "alias_method"
                            ) {
                                let count = if method.as_str() == "alias_method" {
                                    1
                                } else {
                                    args.len()
                                };
                                for arg in args.iter().take(count) {
                                    if let ExprNode::Lit {
                                        value: Literal::Sym { value },
                                    } = &*arg.node
                                    {
                                        overrides.insert(value.clone());
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            let names = model
                .associations()
                .filter_map(|assoc| match assoc {
                    Association::HasMany { name, target, .. } if !overrides.contains(name) => {
                        Some((name.clone(), target.clone()))
                    }
                    _ => None,
                })
                .collect();
            (model.name.clone(), names)
        })
        .collect();
    if assocs.values().all(HashMap::is_empty) {
        return;
    }

    let model_names: HashSet<ClassId> = app.models.iter().map(|m| m.name.clone()).collect();
    // Bare association reads are valid only in instance methods. Class-side
    // methods can define a reader with the same name and a different result.
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                ModelBodyItem::Method { method, .. } => {
                    let self_model =
                        (method.receiver == MethodReceiver::Instance).then_some(&model.name);
                    rewrite(&mut method.body, &assocs, self_model);
                    for param in &mut method.params {
                        if let Some(default) = &mut param.default {
                            rewrite(default, &assocs, self_model);
                        }
                    }
                }
                ModelBodyItem::Association {
                    assoc: Association::HasMany { extension, .. },
                    ..
                } => {
                    for method in extension {
                        let self_model =
                            (method.receiver == MethodReceiver::Instance).then_some(&model.name);
                        rewrite(&mut method.body, &assocs, self_model);
                    }
                }
                ModelBodyItem::Scope { scope, .. } => rewrite(&mut scope.body, &assocs, None),
                ModelBodyItem::Callback { callback, .. } => {
                    if let Some(condition) = &mut callback.condition {
                        rewrite(condition, &assocs, None);
                    }
                }
                ModelBodyItem::Unknown { expr, .. } => rewrite(expr, &assocs, None),
                _ => {}
            }
        }
    }

    let library_names: HashSet<ClassId> = app
        .library_classes
        .iter()
        .map(|lc| lc.name.clone())
        .collect();
    for lc in &mut app.library_classes {
        for method in &mut lc.methods {
            rewrite(&mut method.body, &assocs, None);
            for param in &mut method.params {
                if let Some(default) = &mut param.default {
                    rewrite(default, &assocs, None);
                }
            }
        }
        for (_, value) in &mut lc.constants {
            rewrite(value, &assocs, None);
        }
        for call in &mut lc.unknown_calls {
            rewrite(call, &assocs, None);
        }
    }

    // Controller actions, seeds, and other hook bodies. Models and library
    // classes were handled above with their method receiver information.
    super::for_each_owned_hook_body(app, &mut |owner, body| {
        if owner.is_some_and(|id| model_names.contains(id) || library_names.contains(id)) {
            return;
        }
        rewrite(body, &assocs, None);
    });
    for view in &mut app.views {
        rewrite(&mut view.body, &assocs, None);
    }
    super::for_each_test_body(app, &mut |body| rewrite(body, &assocs, None));
}

fn rewrite(expr: &mut Expr, assocs: &Associations, self_model: Option<&ClassId>) {
    expr.node
        .for_each_child_mut(&mut |child| rewrite(child, assocs, self_model));
    let ExprNode::Send {
        recv: Some(read),
        method,
        ..
    } = &mut *expr.node
    else {
        return;
    };
    if method.as_str() != "new" {
        return;
    }
    let ExprNode::Send {
        recv: owner,
        method: name,
        args,
        block: None,
        ..
    } = &*read.node
    else {
        return;
    };
    if !args.is_empty() {
        return;
    }
    let model = match owner {
        None => self_model,
        Some(owner) if matches!(&*owner.node, ExprNode::SelfRef) => self_model,
        Some(owner) if matches!(&*owner.node, ExprNode::Const { .. }) => None,
        Some(owner) if matches!(&*owner.node, ExprNode::Send { method, .. } if method.as_str() == "class") => {
            None
        }
        Some(owner) => match owner.ty.as_ref().map(Ty::peel_nilable) {
            Some(Ty::Class { id, .. }) => Some(id),
            _ => None,
        },
    };
    let Some(target) = model
        .and_then(|id| assocs.get(id))
        .and_then(|names| names.get(name))
    else {
        return;
    };
    let is_collection = match read.ty.as_ref().map(Ty::peel_nilable) {
        Some(Ty::Array { elem }) => {
            matches!(elem.peel_nilable(), Ty::Class { id, .. } if id == target)
        }
        Some(Ty::Relation { of }) => of == target,
        _ => false,
    };
    if is_collection {
        *method = Symbol::from("build");
    }
}
