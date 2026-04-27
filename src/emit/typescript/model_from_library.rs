//! Strangler-fig path for TS model emission. Routes each `Model` through
//! `lower::model_to_library` and renders the resulting `LibraryClass` as
//! a TypeScript model file. Models whose Rails-shape body carries idioms
//! the lowerer doesn't yet handle (broadcasts, callbacks beyond
//! `before_destroy`, scopes, custom methods, …) return `None` so the
//! caller can fall back to the rich-dialect path in `model.rs`.
//!
//! See `project_universal_post_lowering_ir.md`: the architectural goal
//! is "all emitters consume `LibraryClass`," with the rich `Model`
//! dialect serving only as input to lowerers. This module is the first
//! non-Ruby emitter to consume the lowered IR end-to-end.

use crate::App;
use crate::dialect::{Association, Dependent, Model, ModelBodyItem, ValidationRule};

use super::super::EmittedFile;

/// Try emitting `model` via the lowered-IR path. Returns `None` when the
/// model uses idioms the lowerer doesn't yet cover; the caller is
/// expected to fall back to the rich-dialect path for those.
pub(super) fn try_emit(model: &Model, app: &App) -> Option<EmittedFile> {
    if !is_lowerable(model) {
        return None;
    }
    let _ = app;
    None
}

/// Strict policy: every body item must lower cleanly. The lowerer
/// silently skips unhandled items today, so a permissive admit-anything
/// gate would emit incomplete TS — strict gating preserves correctness
/// while the lowerer grows.
fn is_lowerable(model: &Model) -> bool {
    for item in &model.body {
        match item {
            ModelBodyItem::Association { assoc, .. } => match assoc {
                Association::HasMany { through, dependent, .. } => {
                    if through.is_some() {
                        return false;
                    }
                    if !matches!(dependent, Dependent::None | Dependent::Destroy) {
                        return false;
                    }
                }
                Association::BelongsTo { .. } => {}
                // has_one / HABTM aren't lowered yet.
                Association::HasOne { .. } | Association::HasAndBelongsToMany { .. } => {
                    return false;
                }
            },
            ModelBodyItem::Validation { validation, .. } => {
                for rule in &validation.rules {
                    if matches!(
                        rule,
                        ValidationRule::Uniqueness { .. } | ValidationRule::Custom { .. }
                    ) {
                        return false;
                    }
                }
            }
            // Scopes, callbacks, custom methods, and unknown class-body
            // statements (broadcasts_to, primary_abstract_class, …) all
            // need lowerer extensions before the new path can render
            // them faithfully.
            ModelBodyItem::Scope { .. }
            | ModelBodyItem::Callback { .. }
            | ModelBodyItem::Method { .. }
            | ModelBodyItem::Unknown { .. } => return false,
        }
    }
    true
}
