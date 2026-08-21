//! Callbacks declared in an STI SUBCLASS.
//!
//! `class Rooms::Open < Room; after_save_commit :grant_access_to_all_users; end`
//!
//! An STI subclass declares no table of its own, so it is not ingested
//! as a Model ([`super::sti_scope`] documents that choice and what else
//! follows from it) — it reaches the IR as a `LibraryClass`, and the
//! callback-to-hook synthesis in `model_to_library::markers` walks
//! `app.models`. The declaration therefore landed in `unknown_calls`,
//! preserved and never acted on: the method was emitted and nothing
//! ever called it. campfire's two `Rooms::Open` tests both assert on
//! what that callback grants, and both read `assert_equal failed` with
//! no memberships in the table.
//!
//! The fold is the same one the model path uses — a self-call appended
//! to the hook method of the same name — and it works for a subclass
//! for the reason Ruby dispatch works: `Model.create!` builds through
//! `new(attrs)` at implicit self, so `Rooms::Open.create!` really does
//! produce a `Rooms::Open`, and the runtime's `save_after_validation`
//! finds the subclass's override.
//!
//! ONLY the bare symbol form. An `if:`/`unless:`/`on:` option is
//! declined rather than dropped, matching `push_symbol_callback`'s
//! `condition.is_some()` early return: a callback that runs in the
//! wrong circumstances is worse than one that does not run, and a
//! declined declaration stays visible in `unknown_calls`.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

/// The hook names the framework runtime actually calls, mapped to the
/// method a subclass has to override to be reached. Rails' per-
/// lifecycle commit sugar keeps its own name here because
/// `runtime/ruby/active_record/base.rb` defines a hook of that exact
/// name and fires it in Rails' order — the remapping the model path
/// does is for `after_commit ..., on: <event>`, a spelling this pass
/// declines anyway.
fn hook_method(name: &str) -> Option<&'static str> {
    Some(match name {
        "before_validation" => "before_validation",
        "after_validation" => "after_validation",
        "before_save" => "before_save",
        "after_save" => "after_save",
        "before_create" => "before_create",
        "after_create" => "after_create",
        "before_update" => "before_update",
        "after_update" => "after_update",
        "before_destroy" => "before_destroy",
        "after_destroy" => "after_destroy",
        "after_commit" => "after_commit",
        "after_save_commit" => "after_save_commit",
        "after_create_commit" => "after_create_commit",
        "after_update_commit" => "after_update_commit",
        "after_destroy_commit" => "after_destroy_commit",
        "after_rollback" => "after_rollback",
        _ => return None,
    })
}

pub fn apply_sti_subclass_callbacks(app: &mut App) {
    let models: std::collections::HashSet<ClassId> =
        app.models.iter().map(|m| m.name.clone()).collect();
    for lc in &mut app.library_classes {
        // An STI subclass is a library class whose parent IS a model.
        if !lc.parent.as_ref().is_some_and(|p| models.contains(p)) {
            continue;
        }
        let mut folds: Vec<(&'static str, Vec<Symbol>)> = Vec::new();
        lc.unknown_calls.retain(|call| {
            let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else {
                return true;
            };
            let Some(hook) = hook_method(method.as_str()) else { return true };
            // Every argument must be a bare symbol. A trailing option
            // hash (`if:`, `on:`) means conditions this fold cannot
            // honor, so the whole declaration is left alone.
            let mut targets = Vec::new();
            for a in args {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node else {
                    return true;
                };
                targets.push(value.clone());
            }
            if targets.is_empty() {
                return true;
            }
            folds.push((hook, targets));
            false
        });
        for (hook, targets) in folds {
            for target in targets {
                fold(lc, hook, &target);
            }
        }
    }
}

/// Append `<target>` to the class's `<hook>` method, creating it when
/// absent. Same shape as `model_to_library::markers::fold_into_or_push`,
/// against a `LibraryClass` instead of a `Model`.
fn fold(lc: &mut crate::dialect::LibraryClass, hook: &str, target: &Symbol) {
    let span = crate::span::Span::synthetic();
    let call = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: target.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let hook = Symbol::from(hook);
    if let Some(existing) = lc.methods.iter_mut().find(|m| {
        m.name == hook && m.receiver == crate::dialect::MethodReceiver::Instance
    }) {
        let mut stmts = match &*existing.body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![existing.body.clone()],
        };
        stmts.push(call);
        existing.body = Expr::new(span, ExprNode::Seq { exprs: stmts });
        return;
    }
    lc.methods.push(crate::dialect::MethodDef {
        name: hook,
        receiver: crate::dialect::MethodReceiver::Instance,
        params: Vec::new(),
        body: call,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(lc.name.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
}
