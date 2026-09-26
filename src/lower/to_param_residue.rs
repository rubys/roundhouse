//! `"#{v.to_param}"` on a receiver inference could not type →
//! `"#{ActiveSupport.to_param(v)}"`.
//!
//! ActiveSupport defines `to_param` on Object (`to_s`), true/false/nil,
//! Hash (`to_query`) and Array (the elements' params joined by "/"); a
//! record answers its own. The CRuby lane serves an untyped receiver
//! from the core_ext reopen, but spinel's dispatch for the name has arms
//! for the app's records only, so a builtin value fell through to
//! NoMethodError. lobsters' anonymous story-list cache key is the shape
//! (`opts.merge(page: page).sort.map { |k, v| "#{k}=#{v.to_param}" }`,
//! over `true`, an Integer and a Hash) — every anonymous story list 500'd.
//!
//! Same arrangement as `lower::blank`'s runtime residue: the value is an
//! ARGUMENT to a runtime function that branches on it
//! (`runtime/ruby/active_support_ext.rb`). A receiver typed as one class
//! or as a String keeps its call — its method is known. Only an
//! INTERPOLATED call is routed: that is where `nil.to_param` (nil) and
//! the helper's "" render alike, so the helper can stay String-typed.

use crate::app::App;
use crate::expr::{Expr, ExprNode, InterpPart};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_to_param_residue_lowering(app: &mut App) {
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => walk(&mut action.body),
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => walk(expr),
                _ => {}
            }
        }
    }
    for model in &mut app.models {
        for item in &mut model.body {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                walk(&mut method.body);
            }
        }
    }
    for class in &mut app.library_classes {
        for m in &mut class.methods {
            walk(&mut m.body);
        }
    }
}

/// A receiver whose `to_param` no static method answers.
fn is_residue(ty: Option<&Ty>) -> bool {
    match ty {
        None | Some(Ty::Untyped) | Some(Ty::Var { .. }) => true,
        Some(Ty::Union { variants }) => variants.iter().any(|v| !matches!(v, Ty::Class { .. })),
        _ => false,
    }
}

fn walk(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut walk);
    let ExprNode::StringInterp { parts } = &mut *expr.node else { return };
    for part in parts.iter_mut() {
        let InterpPart::Expr { expr: e } = part else { continue };
        let span = e.span;
        let ExprNode::Send { recv, method, args, block, parenthesized } = &mut *e.node else {
            continue;
        };
        if method.as_str() != "to_param" || !args.is_empty() || block.is_some() {
            continue;
        }
        let Some(r) = recv.as_ref() else { continue };
        if !is_residue(r.ty.as_ref()) {
            continue;
        }
        let receiver = recv.take().unwrap();
        *recv = Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] }));
        *method = Symbol::from("to_param");
        args.push(receiver);
        *parenthesized = true;
        e.ty = Some(Ty::Str);
    }
}
