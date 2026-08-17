//! Ledger a `self.update(<key>: …)` whose key no writer backs.
//!
//! `update` assigns through the model's writable surface
//! ([`writable_field_set`]) — columns, `belongs_to`, `attr_accessor`,
//! `typed_store`, the Attributes API, `has_secure_password`'s plaintext
//! pair, `has_rich_text` attrs, and hand-written `def <f>=`. A key
//! outside that set has nowhere to go, and Rails raises
//! `ActiveModel::UnknownAttributeError` when it happens.
//!
//! The synthesized `update` cannot raise — it iterates the writable set
//! and an unmatched key simply never matches, which is the SILENT DROP
//! that made this pass necessary. Before `synth_update_hash` widened
//! from `table.columns` to the writable set, `users(:david).update(
//! password: "…")` dropped the password and the test that asserted
//! `valid?` afterwards passed having tested nothing. A hollow green is
//! worse than the red; this is the tripwire that keeps the next one from
//! being quiet.
//!
//! **Only self-receiver sites inside a model body are checked.** That is
//! the precise case: the model is known without typing anything, and a
//! bare `update(…)` there cannot be `Hash#update` (Ruby's `merge!`
//! alias), which is what an explicit receiver might be. Sites with an
//! explicit receiver — `users(:david).update(…)` in a test, `webhook&.
//! update!(…)` — need the receiver's type and stay unchecked; ledgered
//! here rather than guessed at.

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_update_writer_check(app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else { continue };
        let writable = crate::lower::model_to_library::writable_field_set(model, table);
        for item in &model.body {
            let crate::dialect::ModelBodyItem::Method { method, .. } = item else { continue };
            check(&method.body, &mut |span, key| {
                if writable.contains(key)
                    || crate::lower::model_to_library::model_defines_writer(model, key)
                {
                    return;
                }
                let kind = DiagnosticKind::LowerResidue {
                    pass: Symbol::from("update_writer_check"),
                    construct: Symbol::from("update"),
                    reason: Symbol::from("no writer"),
                };
                crate::emit::diagnostics::push(Diagnostic {
                    span,
                    severity: Diagnostic::default_severity(&kind),
                    kind,
                    message: format!(
                        "`update` key `{key}` has no writer on `{model_name}` (not a column, \
                         attr_writer, belongs_to, typed_store, has_secure_password, \
                         has_rich_text, or `def {key}=`) — the assignment is dropped; Rails \
                         would raise UnknownAttributeError",
                        key = key.as_str(),
                        model_name = model.name.0.as_str(),
                    ),
                });
            });
        }
    }
}

/// Visit every `update(<literal hash>)` / `update!(…)` with no explicit
/// receiver, reporting each symbol key.
///
/// Rides `map_expr`'s traversal (discarding the rewritten tree) rather
/// than carrying a second copy of the child-visit match — that match is
/// per-`ExprNode`-variant, and a private duplicate is exactly the kind
/// of list that drifts when a variant is added.
fn check(expr: &Expr, report: &mut impl FnMut(crate::span::Span, &Symbol)) {
    // `map_expr`'s closure is `Fn`, not `FnMut`, so keys are collected
    // through a cell and reported after the walk.
    let seen: std::cell::RefCell<Vec<(crate::span::Span, Symbol)>> =
        std::cell::RefCell::new(Vec::new());
    let _ = crate::lower::controller_to_library::util::map_expr(expr, &|e| {
        let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else {
            return None;
        };
        let self_recv =
            recv.is_none() || matches!(recv.as_ref().map(|r| &*r.node), Some(ExprNode::SelfRef));
        if !self_recv || !matches!(method.as_str(), "update" | "update!") || args.len() != 1 {
            return None;
        }
        if let ExprNode::Hash { entries, .. } = &*args[0].node {
            for (k, _v) in entries {
                if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                    seen.borrow_mut().push((e.span, value.clone()));
                }
            }
        }
        None
    });
    for (span, key) in seen.borrow().iter() {
        report(*span, key);
    }
}
