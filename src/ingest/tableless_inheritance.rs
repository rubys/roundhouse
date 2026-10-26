//! A subclass of a tableless model inherits its parent's model DSL.
//!
//! `class Admin::AccountAction < Admin::BaseAction`, where the base
//! includes `ActiveModel::Model` and `ActiveModel::Attributes`, is a
//! tableless model too (`ModelBases::record_tableless`). In Rails
//! (activemodel 8.1.4) the child inherits the parent's attributes,
//! defaults, validations and constructor, and adds its own:
//! `Sub.new(count: "7", extra: "8")` casts both, `Sub.attribute_names`
//! lists the parent's first, a parent validation still runs, and an
//! `attribute` the child redeclares overrides the parent's type and
//! default for the child only.
//!
//! The model path synthesizes one `initialize(attrs)` and one `validate`
//! per class from the class's own body, so the child's would cover only
//! its own declarations and shadow the parent's. This pass prepends the
//! parent's declarations to the child's body instead: its
//! `include ActiveModel::*`, `attribute` (unless the child redeclares the
//! name), `attr_*`, validations and callbacks. Methods and aliases are
//! not copied: the emitted child is still `< Parent`, so Ruby inherits
//! them.

use crate::dialect::{Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

use crate::App;

pub(super) fn inherit_tableless_parents(app: &mut App) {
    let index: std::collections::HashMap<String, usize> =
        app.models.iter().enumerate().map(|(i, m)| (m.name.0.as_str().to_string(), i)).collect();
    let resolved: Vec<Option<usize>> = app
        .models
        .iter()
        .map(|m| resolve_parent(&index, m.name.0.as_str(), m.parent.as_ref()?.0.as_str()))
        .collect();
    // Each child's tableless parent. A parent qualifies by its own body
    // or, in the middle of a chain, by having a tableless parent itself;
    // closed over until nothing changes, so file order doesn't matter.
    let mut tableless: Vec<bool> = app.models.iter().map(|m| m.parent.is_none() && is_tableless_parent(m)).collect();
    loop {
        let mut added = false;
        for (i, m) in app.models.iter().enumerate() {
            if !tableless[i] && !defines_initialize(m) && resolved[i].is_some_and(|p| tableless[p]) {
                tableless[i] = true;
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    let parent_of: Vec<Option<usize>> = resolved.iter().map(|p| p.filter(|&p| tableless[p])).collect();
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = parent_of[i] {
            d += 1;
            i = p;
            if d > parent_of.len() {
                break;
            }
        }
        d
    };
    let mut order: Vec<usize> = (0..app.models.len()).filter(|&i| parent_of[i].is_some()).collect();
    order.sort_by_key(|&i| depth(i));
    for child in order {
        let parent = parent_of[child].expect("filtered above");
        let own_attributes = attribute_names(&app.models[child].body);
        let inherited: Vec<ModelBodyItem> = app.models[parent]
            .body
            .iter()
            .filter(|item| inherited_item(item, &own_attributes))
            .cloned()
            .map(without_layout)
            .collect();
        let body = std::mem::take(&mut app.models[child].body);
        app.models[child].body = inherited.into_iter().chain(body).collect();
    }
}

/// `parent` as written in `child`'s file, resolved the way Ruby's
/// constant lookup walks the child's enclosing modules.
fn resolve_parent(index: &std::collections::HashMap<String, usize>, child: &str, parent: &str) -> Option<usize> {
    if !parent.contains("::") {
        let mut segs: Vec<&str> = child.split("::").collect();
        segs.pop();
        while !segs.is_empty() {
            if let Some(&i) = index.get(&format!("{}::{}", segs.join("::"), parent)) {
                return Some(i);
            }
            segs.pop();
        }
    }
    index.get(parent).copied()
}

/// The test `ModelBases::record_tableless` applies to a superclass-less
/// class's source, on its ingested body.
fn is_tableless_parent(model: &Model) -> bool {
    let includes = model.body.iter().any(|item| {
        active_model_include(item).is_some_and(|leaf| leaf == "Model" || leaf == "Validations")
    });
    includes && !defines_initialize(model)
}

fn defines_initialize(model: &Model) -> bool {
    model.body.iter().any(|item| {
        matches!(item, ModelBodyItem::Method { method, .. } if method.name.as_str() == "initialize")
    })
}

fn inherited_item(item: &ModelBodyItem, own_attributes: &[Symbol]) -> bool {
    match item {
        ModelBodyItem::Validation { .. } | ModelBodyItem::Callback { .. } => true,
        ModelBodyItem::Unknown { expr, .. } => {
            if active_model_include_expr(expr).is_some() {
                return true;
            }
            let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return false };
            match method.as_str() {
                "attr_accessor" | "attr_reader" | "attr_writer" => true,
                "attribute" => attribute_name(args).is_some_and(|name| !own_attributes.contains(&name)),
                _ => false,
            }
        }
        _ => false,
    }
}

fn without_layout(item: ModelBodyItem) -> ModelBodyItem {
    match item {
        ModelBodyItem::Validation { validation, span, .. } => {
            ModelBodyItem::Validation { validation, span, leading_comments: Vec::new(), leading_blank_line: false }
        }
        ModelBodyItem::Callback { callback, span, .. } => {
            ModelBodyItem::Callback { callback, span, leading_comments: Vec::new(), leading_blank_line: false }
        }
        ModelBodyItem::Unknown { expr, .. } => {
            ModelBodyItem::Unknown { expr, leading_comments: Vec::new(), leading_blank_line: false }
        }
        other => other,
    }
}

fn attribute_names(body: &[ModelBodyItem]) -> Vec<Symbol> {
    body.iter()
        .filter_map(|item| {
            let ModelBodyItem::Unknown { expr, .. } = item else { return None };
            let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return None };
            (method.as_str() == "attribute").then(|| attribute_name(args)).flatten()
        })
        .collect()
}

fn attribute_name(args: &[Expr]) -> Option<Symbol> {
    match args.first().map(|a| &*a.node) {
        Some(ExprNode::Lit { value: crate::expr::Literal::Sym { value } }) => Some(value.clone()),
        _ => None,
    }
}

fn active_model_include(item: &ModelBodyItem) -> Option<&str> {
    let ModelBodyItem::Unknown { expr, .. } = item else { return None };
    active_model_include_expr(expr)
}

/// `include ActiveModel::<leaf>`: the leaf.
fn active_model_include_expr(expr: &Expr) -> Option<&str> {
    let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return None };
    if method.as_str() != "include" {
        return None;
    }
    args.iter().find_map(|a| match &*a.node {
        ExprNode::Const { path } if path.len() == 2 && path[0].as_str() == "ActiveModel" => Some(path[1].as_str()),
        _ => None,
    })
}
