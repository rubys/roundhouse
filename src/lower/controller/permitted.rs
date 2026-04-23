//! Strong-params extraction and recognition — pulls the permitted-
//! field list out of a controller's `<resource>_params` helper,
//! recognizes `Model.new(resource_params)` and `x.update(
//! resource_params)` patterns, and provides a fallback attribute
//! list when the helper can't be parsed.

use crate::App;
use crate::dialect::{Controller, ControllerBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use super::util::{is_params_expr, singularize_to_model};

/// Dig the `<resource>_params` private helper out of the controller
/// and extract the permitted field names from its
/// `params.expect(scope: [:field1, :field2])` body. Returns `None`
/// when the helper doesn't exist or the body doesn't match the
/// canonical Rails scaffold shape — callers fall back to
/// `default_permitted_fields`.
pub fn permitted_fields_for(
    controller: &Controller,
    resource: &str,
) -> Option<Vec<String>> {
    let helper_name = format!("{resource}_params");
    let action = controller.body.iter().find_map(|item| match item {
        ControllerBodyItem::Action { action, .. }
            if action.name.as_str() == helper_name =>
        {
            Some(action)
        }
        _ => None,
    })?;
    extract_permitted_from_expr(&action.body)
}

/// Walk an expression looking for a `params.expect(<scope>: [:f1,
/// :f2])` call and return the field name list. Recurses into Seqs
/// so a helper with a guard or local first still resolves.
pub fn extract_permitted_from_expr(expr: &Expr) -> Option<Vec<String>> {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*expr.node {
        if method.as_str() == "expect" && is_params_expr(r) {
            if let Some(arg) = args.first() {
                if let ExprNode::Hash { entries, .. } = &*arg.node {
                    if let Some((_, value)) = entries.first() {
                        if let ExprNode::Array { elements, .. } = &*value.node {
                            let fields: Vec<String> = elements
                                .iter()
                                .filter_map(|e| match &*e.node {
                                    ExprNode::Lit {
                                        value: Literal::Sym { value },
                                    } => Some(value.as_str().to_string()),
                                    _ => None,
                                })
                                .collect();
                            if !fields.is_empty() {
                                return Some(fields);
                            }
                        }
                    }
                }
            }
        }
    }
    if let ExprNode::Seq { exprs } = &*expr.node {
        for e in exprs {
            if let Some(v) = extract_permitted_from_expr(e) {
                return Some(v);
            }
        }
    }
    None
}

/// True when `expr` is a zero-arg, zero-block, no-receiver call to
/// the resource's strong-params helper — e.g. `post_params` when
/// the resource is `post`, `article_params` when it's `article`.
/// Rails convention: every scaffold controller defines a
/// `<resource>_params` private method that returns the permitted
/// form fields.
pub fn is_resource_params_call(expr: &Expr, resource: &str) -> bool {
    let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else {
        return false;
    };
    if !args.is_empty() || block.is_some() {
        return false;
    }
    let expected = format!("{resource}_params");
    method.as_str() == expected
}

/// Recognize a Create-scaffold instantiation: either
///   `Model.new(<resource>_params)`                (top-level)
/// or
///   `@parent.<assoc>.build(<resource>_params)`    (nested resource)
/// where the target class resolves to one of the app's known
/// models. Returns the target class symbol so the walker can emit
/// `new Class()` plus per-target per-field assigns. Used by every
/// emitter's Assign handler — the strong-params expansion shape
/// differs per target but the recognition is target-neutral.
pub fn model_new_with_strong_params(
    value: &Expr,
    known_models: &[Symbol],
    resource: &str,
) -> Option<Symbol> {
    let ExprNode::Send { recv, method, args, .. } = &*value.node else {
        return None;
    };
    if args.len() != 1 || !is_resource_params_call(&args[0], resource) {
        return None;
    }
    let r = recv.as_ref()?;
    // Top-level: `Model.new(...)` on a Const receiver resolving to
    // a known model.
    if method.as_str() == "new" {
        if let ExprNode::Const { path } = &*r.node {
            let class = path.last()?;
            known_models.iter().find(|m| *m == class)?;
            return Some(class.clone());
        }
    }
    // Nested: `<parent>.<assoc>.build(...)` — the inner Send's
    // method (a plural association like `comments`) singularizes
    // to a known model (`Comment`).
    if method.as_str() == "build" {
        if let ExprNode::Send {
            recv: Some(_),
            method: assoc_method,
            args: inner_args,
            ..
        } = &*r.node
        {
            if inner_args.is_empty() {
                if let Some(target) =
                    singularize_to_model(assoc_method.as_str(), known_models)
                {
                    return Some(target);
                }
            }
        }
    }
    None
}

/// Recognize the Update-scaffold pattern: `<recv>.update(
/// <resource>_params)` on a non-Const receiver (a local, not a
/// class method). Returns the receiver `Expr` so the walker can
/// emit `<recv>.save` plus hoisted per-field conditional assigns.
pub fn update_with_strong_params<'a>(
    cond: &'a Expr,
    resource: &str,
) -> Option<&'a Expr> {
    let ExprNode::Send { recv, method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "update" || args.len() != 1 {
        return None;
    }
    let r = recv.as_ref()?;
    if matches!(&*r.node, ExprNode::Const { .. }) {
        return None;
    }
    if !is_resource_params_call(&args[0], resource) {
        return None;
    }
    Some(r)
}

/// Fallback permitted-field list when the `<resource>_params` helper
/// isn't recognizable. Returns the model's non-id, non-timestamp,
/// non-foreign-key attributes — a safe default that matches what
/// the Rails scaffold generator would produce.
pub fn default_permitted_fields(app: &App, model_class: &str) -> Vec<String> {
    let Some(model) = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == model_class)
    else {
        return Vec::new();
    };
    model
        .attributes
        .fields
        .keys()
        .map(|k| k.as_str().to_string())
        .filter(|name| {
            name != "id"
                && name != "created_at"
                && name != "updated_at"
                && !name.ends_with("_id")
        })
        .collect()
}
