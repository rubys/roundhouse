//! `record.errors&.full_messages` → `record.errors.full_messages`, and the
//! same for every link of a `&.` chain whose receiver cannot be nil.
//!
//! `&.` binds a receiver that is not a plain read to a local first
//! (`(__safe_nav1 = record.errors) && __safe_nav1.full_messages`, see
//! `ingest::expr`), and the lowerings that rewrite a chain match its
//! receiver as a send: every `errors` lowering, `association(:x).loaded?`
//! (`assoc_loaded`), a has_many reader followed by a query method. The
//! local hid the site from all of them, so `full_messages` stayed a call
//! on the Array of full messages, and `association` a call nothing
//! defines. When the receiver cannot be nil the guard never fires, and
//! the send reads it directly. `record&.errors&.m` keeps the guard on
//! `record`: `record && record.errors.m`.
//!
//! A receiver that cannot be nil, by its type, never by its name:
//! - `errors` of `ActiveModel::Errors`, never an app's own `errors` that
//!   may answer nil (an untyped `errors&.full_messages` is folded by
//!   `errors_full_messages`, where the hop is an identity);
//! - `association(:name)` on a record of an app model (Rails raises for
//!   an unknown name, it never answers nil);
//! - a has_many reader on a record of a model that declares it
//!   (`owner&.gadgets&.where(...)`), a value typed as a relation, and the
//!   query methods chained on either.

use crate::app::App;
use crate::expr::{BoolOpKind, Expr, ExprNode, LValue, Literal};
use crate::ident::ClassId;
use crate::ty::Ty;

/// Every app model, with its has_many names.
type Models = std::collections::HashMap<ClassId, std::collections::HashSet<crate::ident::Symbol>>;

/// Rails' query methods: each answers a relation, never nil.
const QUERY_METHODS: &[&str] = &[
    "where", "not", "rewhere", "order", "reorder", "reverse_order", "limit", "offset", "includes",
    "preload", "eager_load", "joins", "left_joins", "left_outer_joins", "distinct", "group", "having",
    "unscope", "references", "readonly", "all", "none",
];

pub fn apply_errors_safe_nav_lowering(app: &mut App) {
    let models: Models = super::assoc_loaded::has_many_by_model(app);
    let rewrite = &mut |e: &mut Expr| rewrite(e, &models);
    super::for_each_hook_body(app, rewrite);
    super::for_each_test_body(app, rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn is_model_errors(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { method, args, block: None, .. } if method.as_str() == "errors" && args.is_empty())
        && matches!(&e.ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Errors")
}

/// A record of an app model: `Widget`, or `Widget | nil` (the guard in
/// front has already ruled nil out).
fn is_model_record(ty: Option<&Ty>, models: &Models) -> bool {
    !record_models(ty, models).is_empty()
}

/// The models a record type names: `Widget`, or each of `Widget | Gadget
/// | nil`. Empty when any non-nil member is not an app model.
fn record_models<'a>(ty: Option<&'a Ty>, models: &Models) -> Vec<&'a ClassId> {
    match ty {
        Some(Ty::Class { id, args }) if args.is_empty() && models.contains_key(id) => vec![id],
        Some(Ty::Union { variants }) => {
            let mut out = Vec::new();
            for v in variants.iter().filter(|v| !matches!(v, Ty::Nil)) {
                match v {
                    Ty::Class { id, args } if args.is_empty() && models.contains_key(id) => out.push(id),
                    _ => return Vec::new(),
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Whether `e` answers a value that is never nil (nor false), so `&.`
/// after it never short-circuits.
fn never_nil(e: &Expr, models: &Models) -> bool {
    if is_model_errors(e) || matches!(&e.ty, Some(Ty::Relation { .. })) {
        return true;
    }
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else { return false };
    match method.as_str() {
        "association" => {
            matches!(args.as_slice(), [arg] if matches!(&*arg.node, ExprNode::Lit { value: Literal::Sym { .. } }))
                && is_model_record(recv.ty.as_ref(), models)
        }
        // A has_many reader: every model the record may be declares it.
        name if args.is_empty() => {
            let owners = record_models(recv.ty.as_ref(), models);
            !owners.is_empty() && owners.iter().all(|id| models[*id].iter().any(|n| n.as_str() == name))
        }
        // A query method on a has_many reader or another relation.
        name => QUERY_METHODS.contains(&name) && never_nil(recv, models),
    }
}

fn rewrite(expr: &mut Expr, models: &Models) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, models));
    let ExprNode::BoolOp { op: BoolOpKind::And, left, right, .. } = &mut *expr.node else { return };
    let ExprNode::Assign { target: LValue::Var { name: temp, .. }, value } = &mut *left.node else { return };
    let ExprNode::Send { recv: Some(read), .. } = &*right.node else { return };
    if !matches!(&*read.node, ExprNode::Var { name, .. } if name == temp) {
        return;
    }
    if never_nil(value, models) {
        let value = value.clone();
        let mut send = right.clone();
        let ExprNode::Send { recv, .. } = &mut *send.node else { unreachable!() };
        *recv = Some(value);
        *expr = send;
        return;
    }
    // `record&.errors` is `record && record.errors`; the guard stays on `record`.
    let ExprNode::BoolOp { op: BoolOpKind::And, left: guard, right: inner, .. } = &*value.node else { return };
    if !never_nil(inner, models) {
        return;
    }
    let (guard, inner) = (guard.clone(), inner.clone());
    let mut send = right.clone();
    let ExprNode::Send { recv, .. } = &mut *send.node else { unreachable!() };
    *recv = Some(inner);
    let ExprNode::BoolOp { left, right, .. } = &mut *expr.node else { unreachable!() };
    *left = guard;
    *right = send;
}
