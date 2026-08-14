//! Rails-API broadcast calls in ORDINARY method bodies.
//!
//! `after_create_commit { article.broadcast_replace_to … }` — a broadcast
//! written inside a callback block — is rewritten where callbacks are
//! synthesized (`model_to_library::markers`). campfire writes its
//! broadcasts the other way: plain methods on a concern,
//!
//! ```text
//! module Message::Broadcasts
//!   def broadcast_create
//!     broadcast_append_to room, :messages, target: [ room, :messages ]
//!   end
//! end
//! ```
//!
//! which no callback walk ever visits. This pass covers both homes those
//! methods can have — a model's own body, and a CONCERN MODULE emitted
//! beside it.
//!
//! The concern half is the reason this is a pass rather than another hook
//! inside `model_to_library`: `Message::Broadcasts` is a library class,
//! not a model, so the rewrite has to resolve its owner (`Message`) from
//! the qualified name to know which associations exist and whose partial
//! the payload renders. A concern nested under a model can only belong to
//! that model — the lexical nesting IS the ownership, which
//! `qualify_relative_model_includes` already relies on.

use crate::app::App;
use crate::dialect::{Model, ModelBodyItem};
use crate::ident::ClassId;

use super::model_to_library::broadcasts::rewrite_rails_broadcast_calls;

pub(crate) fn apply_broadcast_calls_lowering(app: &mut App) {
    // The rewrite reads the owner while its methods are being replaced,
    // so it works from a snapshot. Models are small and this runs once.
    let owners: Vec<Model> = app.models.clone();

    for model in &mut app.models {
        let Some(owner) = owners.iter().find(|m| m.name == model.name) else { continue };
        for item in &mut model.body {
            let ModelBodyItem::Method { method, .. } = item else { continue };
            method.body = rewrite_rails_broadcast_calls(method.body.clone(), owner);
        }
    }

    for lc in &mut app.library_classes {
        let Some(owner) = enclosing_model(&lc.name, &owners) else { continue };
        for method in &mut lc.methods {
            method.body = rewrite_rails_broadcast_calls(method.body.clone(), owner);
        }
    }
}

/// `Message::Broadcasts` → the `Message` model, when there is one. Only
/// the immediately-enclosing segment counts: a deeper nesting
/// (`Message::Attachment::Thing`) still belongs to the model it is
/// nested under, which is the first segment.
fn enclosing_model<'a>(name: &ClassId, models: &'a [Model]) -> Option<&'a Model> {
    let outer = name.0.as_str().split("::").next()?;
    if outer == name.0.as_str() {
        return None;
    }
    models.iter().find(|m| m.name.0.as_str() == outer)
}
