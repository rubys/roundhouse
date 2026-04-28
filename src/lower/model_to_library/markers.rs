//! Unknown body items recognized as Rails markers. Most Unknowns stay
//! dropped (they're emitter responsibility or future-lowerer work), but
//! a small set carry semantics that translate cleanly into method
//! definitions on the lowered class.
//!
//! Block-form lifecycle callbacks: `after_create_commit { … }` etc. They
//! surface as Unknown body items (parse_callback rejects them — no
//! symbol target, just a block). Lowered to a `def hook_name; <block-
//! body>; end`. Multiple sources can target the same hook (block-form
//! callback + broadcasts_to expansion + dependent: :destroy cascade);
//! when this lowering finds an existing method with the matching name
//! it folds the block body into that method's Seq, preserving source
//! order across sources.

use crate::dialect::{MethodDef, MethodReceiver, Model, ModelBodyItem};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;

use super::seq;

/// `primary_abstract_class` marks a model as the abstract base of a Rails
/// app. Lowered to `def self.abstract?; true; end` — the explicit form
/// spinel-blog's runtime expects.
pub(super) fn push_unknown_marker_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for item in &model.body {
        if let ModelBodyItem::Unknown { expr, .. } = item {
            if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
                if args.is_empty() && method.as_str() == "primary_abstract_class" {
                    methods.push(MethodDef {
                        name: Symbol::from("abstract?"),
                        receiver: MethodReceiver::Class,
                        params: Vec::new(),
                        body: Expr::new(
                            Span::synthetic(),
                            ExprNode::Lit { value: Literal::Bool { value: true } },
                        ),
                        signature: None,
                        effects: EffectSet::default(),
                        enclosing_class: Some(model.name.0.clone()),
                    });
                }
            }
        }
    }
}

/// Look up an existing `Method` named `hook_name` and append `call` to
/// its body's Seq, OR push a new method with `call` as the body. The
/// fold preserves source order; broadcasts_to runs first so its calls
/// lead any block-form callback bodies that the next pass would add.
pub(super) fn fold_into_or_push(methods: &mut Vec<MethodDef>, model: &Model, hook_name: &str, call: Expr) {
    let hook = Symbol::from(hook_name);
    if let Some(existing) = methods.iter_mut().find(|m| m.name == hook) {
        let mut stmts = match &*existing.body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![existing.body.clone()],
        };
        stmts.push(call);
        existing.body = seq(stmts);
    } else {
        methods.push(MethodDef {
            name: hook,
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body: call,
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
        });
    }
}

/// Lifecycle hook names that appear as block-form Unknown items. Names
/// not in this set fall through to plain Unknown (they're future
/// lowerer or emit work). Includes the `_commit` variants Rails sugar
/// adds beyond the raw `after_commit` hook in `CallbackHook`.
const BLOCK_CALLBACK_HOOKS: &[&str] = &[
    "before_validation",
    "after_validation",
    "before_save",
    "after_save",
    "before_create",
    "after_create",
    "before_update",
    "after_update",
    "before_destroy",
    "after_destroy",
    "after_commit",
    "after_rollback",
    "after_create_commit",
    "after_update_commit",
    "after_destroy_commit",
    "after_save_commit",
];

pub(super) fn push_block_callback_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*expr.node else {
            continue;
        };
        if !args.is_empty() {
            continue;
        }
        let hook = method.as_str();
        if !BLOCK_CALLBACK_HOOKS.contains(&hook) {
            continue;
        }
        let ExprNode::Lambda { body: lambda_body, .. } = &*block.node else {
            continue;
        };

        let hook_sym = method.clone();
        if let Some(existing) = methods.iter_mut().find(|m| m.name == hook_sym) {
            // Fold this block's body into the existing method, preserving
            // source order (existing body's stmts first, then this block's).
            let mut stmts = match &*existing.body.node {
                ExprNode::Seq { exprs } => exprs.clone(),
                _ => vec![existing.body.clone()],
            };
            match &*lambda_body.node {
                ExprNode::Seq { exprs } => stmts.extend(exprs.clone()),
                _ => stmts.push(lambda_body.clone()),
            }
            existing.body = seq(stmts);
        } else {
            methods.push(MethodDef {
                name: hook_sym,
                receiver: MethodReceiver::Instance,
                params: Vec::new(),
                body: lambda_body.clone(),
                signature: None,
                effects: EffectSet::default(),
                enclosing_class: Some(model.name.0.clone()),
            });
        }
    }
}
