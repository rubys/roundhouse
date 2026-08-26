//! ActiveSupport's `Enumerable#including` → `<recv>.to_a + [args]`.
//!
//! `including(*elements)` is a core_ext one-liner (`dup.concat(elements)`
//! on Array, `to_a + elements` on Enumerable) and hits the same wall as
//! `exclude?` next door: no transpiled runtime can reopen the builtin,
//! so the call has to become the expression it stands for.
//!
//! `.to_a` on the receiver rather than a bare `+`, because the name is
//! Enumerable's and reaches BOTH shapes campfire writes it on — a plain
//! Array (`params.fetch(:user_ids, []).including(Current.user.id)`) and
//! a relation-rooted chain. `to_a` is identity on the first and
//! materializes the second, which is exactly what Rails' Enumerable
//! definition does; rewriting to a bare `+` would be right for the Array
//! and a missing-method for the relation.
//!
//! Args become one Array literal, so the multi-element form
//! (`xs.including(a, b)`) is the same rewrite as the single. Rails
//! flattens one level for the splat form; nothing in the corpus passes
//! a splat, and flattening a genuine Array ELEMENT would be wrong, so
//! the args ride as written.
//!
//! Stands down wholesale if any app class defines its own `including`,
//! for the reason `exclude_predicate` gives: `including` sites are on
//! collections, so a receiver type rarely names the defining class, and
//! a global opt-out is the honest reading of "this app means something
//! else by that name".

use crate::app::App;
use crate::expr::{ArrayStyle, Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_including_lowering(app: &mut App) {
    if app_defines_including(app) {
        return;
    }
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

/// Does any app class define its own `including`? Then the name doesn't
/// mean ActiveSupport's and the pass stands down.
fn app_defines_including(app: &App) -> bool {
    let is_including = |n: &Symbol| n.as_str() == "including";
    let in_model = app.models.iter().any(|m| {
        m.body.iter().any(|item| match item {
            crate::dialect::ModelBodyItem::Method { method, .. } => is_including(&method.name),
            crate::dialect::ModelBodyItem::Scope { scope, .. } => is_including(&scope.name),
            _ => false,
        })
    });
    let in_library = app
        .library_classes
        .iter()
        .any(|lc| lc.methods.iter().any(|m| is_including(&m.name)));
    let in_controller = app.controllers.iter().any(|c| {
        c.body.iter().any(|item| match item {
            crate::dialect::ControllerBodyItem::Action { action, .. } => is_including(&action.name),
            _ => false,
        })
    });
    in_model || in_library || in_controller
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "including" || block.is_some() || args.is_empty() {
        return;
    }
    let span = expr.span;
    // The type `to_a` lands on, read off the receiver we are rewriting
    // (`materialized_to_a` below). `None` when no arm of the receiver
    // can answer — a receiver whose surface we cannot see is one we
    // cannot claim `to_a` for, and leaving it unstamped keeps it on the
    // ledger where it belongs.
    let materialized = recv.ty.as_ref().and_then(materialized_to_a);
    let elem_ty = match &materialized {
        Some(Ty::Array { elem }) => (**elem).clone(),
        _ => Ty::Untyped,
    };
    let mut elements = Expr::new(
        span,
        ExprNode::Array { elements: std::mem::take(args), style: ArrayStyle::Brackets },
    );
    if materialized.is_some() {
        elements.ty = Some(Ty::Array { elem: Box::new(elem_ty) });
    }
    let mut to_a = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv.clone()),
            method: Symbol::from("to_a"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    to_a.ty = materialized.clone();
    *expr.node = ExprNode::Send {
        recv: Some(to_a),
        method: Symbol::from("+"),
        args: vec![elements],
        block: None,
        parenthesized: false,
    };
    expr.ty = materialized;
}

/// The type `to_a` lands on, or `None` when the receiver's surface
/// doesn't say. A union answers from whichever arm can — the same
/// policy the body-typer's union dispatch uses, where arms that decline
/// are dropped and one resolving arm answers. campfire's
/// `params.fetch(:user_ids, []).including(Current.user.id)` types
/// `Array[…] | Str` because the params model calls every value a `Str`
/// (`Roundhouse::ParamValue` is the runtime type, not one the analyzer
/// carries); the Array arm is the one that answers, and leaving the
/// whole union unstamped put a `to_a` error on working code.
fn materialized_to_a(t: &Ty) -> Option<Ty> {
    match t {
        Ty::Array { .. } => Some(t.clone()),
        Ty::Relation { of } => Some(Ty::Array {
            elem: Box::new(Ty::Class { id: of.clone(), args: vec![] }),
        }),
        Ty::Union { variants } => variants.iter().find_map(materialized_to_a),
        _ => None,
    }
}
