//! `<hash-or-array>.to_json` → `JSON.generate(<hash-or-array>)`.
//!
//! `Hash#to_json` is a core_ext reopen — Ruby's json library adds it,
//! ActiveSupport replaces it — and a reopened builtin is the one shape
//! no strict target can host and spinel cannot dispatch on. The value
//! it produces has a home already: the bundled JSON package every
//! emitted tree requires through `runtime/json_impl.rb`, whose
//! `generate` takes the same collection and answers the same String.
//! Same move as `parameterize` → `Inflector.parameterize`, and it
//! spends no new runtime surface.
//!
//! campfire's `Webhook#payload` builds a nested Hash and ends
//! `}.to_json`, which is the whole request body a bot receives — an
//! unresolved call there is not a late NoMethodError on some route, it
//! is every webhook delivery.
//!
//! GATE: a receiver the analyzer typed `Ty::Hash` or `Ty::Array`. Those
//! are exactly what `JSON.generate` serializes directly. A MODEL
//! receiver is deliberately excluded — `record.to_json` is Rails'
//! `as_json`-then-encode, which the `as_json_*` passes own and which
//! answers a different string (the model's declared shape, not its
//! ivars) — and so is an untyped one, where the receiver could be
//! anything and the rewrite would be a guess.
//!
//! DIVERGENCE: ActiveSupport's `to_json` walks `as_json` first, so a
//! Time value renders in Rails' ISO-8601 form where `JSON.generate`
//! would refuse it. The corpus receiver holds strings, integers and
//! nested hashes of the same, and a value JSON cannot encode raises
//! rather than rendering wrongly. Recorded in docs/pipeline/runtime.md.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_to_json_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    rewrite_node(expr);
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    rewrite_with(expr, true);
}

/// `rewrite_node` without the deferral, for `as_json_poro`'s fallback.
pub(crate) fn rewrite_node_now(expr: &mut Expr) {
    rewrite_with(expr, false);
}

fn rewrite_with(expr: &mut Expr, defer: bool) {
    let span = expr.span;
    let ExprNode::Send { recv, method, args, block, parenthesized } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "to_json" || !args.is_empty() || block.is_some() {
        return;
    }
    if !recv
        .as_ref()
        .is_some_and(|r| matches!(r.ty.as_ref(), Some(Ty::Hash { .. } | Ty::Array { .. })))
    {
        return;
    }
    // Not yet: a value holding a record, Time or Date is `lower::as_json_poro`'s to write down, which falls back here when it cannot.
    if defer && recv.as_ref().and_then(|r| r.ty.as_ref()).is_some_and(holds_object) {
        return;
    }
    let receiver = finite_floats(recv.take().expect("checked above"));
    *recv = Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("JSON")] }));
    *method = Symbol::from("generate");
    args.push(receiver);
    *parenthesized = true;
    expr.ty = Some(Ty::Str);
}


fn holds_object(ty: &Ty) -> bool {
    match ty {
        Ty::Class { .. } | Ty::Relation { .. } | Ty::Time | Ty::Date => true,
        Ty::Array { elem } => holds_object(elem),
        Ty::Hash { value, .. } => holds_object(value),
        Ty::Union { variants } => variants.iter().any(holds_object),
        _ => false,
    }
}

/// `value` with each Float leaf a literal or a typed Array reaches wrapped in `JsonBuilder.finite_float`, so `JSON.generate` writes NaN and the infinities `null`, as Rails does.
pub(crate) fn finite_floats(value: Expr) -> Expr {
    let span = value.span;
    let ty = value.ty.clone();
    match *value.node {
        ExprNode::Hash { entries, kwargs } => {
            let entries = entries.into_iter().map(|(k, v)| (k, finite_floats(v))).collect();
            let mut out = Expr::new(span, ExprNode::Hash { entries, kwargs });
            out.ty = ty.as_ref().map(nullable_floats);
            out
        }
        ExprNode::Array { elements, style } => {
            let elements = elements.into_iter().map(finite_float_element).collect();
            let mut out = Expr::new(span, ExprNode::Array { elements, style });
            // Not the literal's own type: its Float elements are `Float?` now, and Go builds the slice from this type.
            out.ty = ty.as_ref().map(nullable_floats);
            out
        }
        node => {
            let value = Expr { node: Box::new(node), ..value };
            match ty.as_ref() {
                // A literal too, though finite: a Hash of `Float?` and bare `Float` values has no one value type on Rust.
                Some(t) if float_leaf(t) => finite_float_call(value),
                Some(Ty::Array { elem }) if float_leaf(elem) => {
                    let item = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("item") });
                    let item = Expr { ty: Some((**elem).clone()), ..item };
                    let block = Expr::new(
                        span,
                        ExprNode::Lambda {
                            extra_params: Vec::new(),
                            rest_param: None,
                            params: vec![Symbol::from("item")],
                            block_param: None,
                            body: finite_float_call(item),
                            block_style: crate::expr::BlockStyle::Brace,
                        },
                    );
                    let mut mapped = Expr::new(
                        span,
                        ExprNode::Send { recv: Some(value), method: Symbol::from("map"), args: vec![], block: Some(block), parenthesized: true },
                    );
                    mapped.ty = Some(Ty::Array { elem: std::sync::Arc::new(nullable_float()) });
                    mapped
                }
                _ => value,
            }
        }
    }
}

/// An element of an Array literal; a splat's operand, not the splat itself, is what holds the Floats.
fn finite_float_element(e: Expr) -> Expr {
    let span = e.span;
    let ty = e.ty.clone();
    match *e.node {
        ExprNode::Splat { value } => {
            let mut out = Expr::new(span, ExprNode::Splat { value: finite_floats(value) });
            out.ty = ty.as_ref().map(nullable_floats);
            out
        }
        node => finite_floats(Expr { node: Box::new(node), ..e }),
    }
}

/// `ty` with each Float a payload can hold made `Float?`, as `finite_floats` leaves its values.
fn nullable_floats(ty: &Ty) -> Ty {
    match ty {
        Ty::Float => nullable_float(),
        Ty::Array { elem } => Ty::Array { elem: std::sync::Arc::new(nullable_floats(elem)) },
        Ty::Hash { key, value } => Ty::Hash { key: key.clone(), value: std::sync::Arc::new(nullable_floats(value)) },
        Ty::Union { variants } => {
            let mut out: Vec<Ty> = Vec::new();
            for v in variants.iter() {
                let v = if matches!(v, Ty::Float) { Ty::Float } else { nullable_floats(v) };
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            if variants.iter().any(|v| matches!(v, Ty::Float)) && !out.contains(&Ty::Nil) {
                out.push(Ty::Nil);
            }
            Ty::Union { variants: out.into() }
        }
        other => other.clone(),
    }
}

fn float_leaf(ty: &Ty) -> bool {
    match ty {
        Ty::Float => true,
        Ty::Union { variants } => {
            variants.iter().any(|v| matches!(v, Ty::Float)) && variants.iter().all(|v| matches!(v, Ty::Float | Ty::Nil))
        }
        _ => false,
    }
}

fn nullable_float() -> Ty {
    Ty::Union { variants: vec![Ty::Float, Ty::Nil].into() }
}

fn finite_float_call(value: Expr) -> Expr {
    let span = value.span;
    // Cast to `Float?`: Rust's param is `Option<f64>`, and the Cast is what its emitter reads as `Some(..)`.
    let value = if matches!(value.ty, Some(Ty::Float)) {
        let mut cast = Expr::new(span, ExprNode::Cast { value, target_ty: nullable_float() });
        cast.ty = Some(nullable_float());
        cast
    } else {
        value
    };
    let mut out = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("JsonBuilder")] })),
            method: Symbol::from("finite_float"),
            args: vec![value],
            block: None,
            parenthesized: true,
        },
    );
    out.ty = Some(nullable_float());
    out
}
