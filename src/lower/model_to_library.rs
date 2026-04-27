//! Lower a Rails-shape `Model` (with associations, validations, callbacks,
//! Schema-derived columns) into a post-lowering `LibraryClass` whose body
//! is a flat sequence of `MethodDef`s — the universal IR shape every
//! emitter consumes (see `project_universal_post_lowering_ir.md`).
//!
//! The output target is `fixtures/spinel-blog/app/models/<model>.rb`:
//! explicit method bodies (`def title; @title; end`, `def comments;
//! Comment.where(article_id: @id); end`, `def validate;
//! validates_presence_of(:title) { @title }; end`), no Rails DSL.
//!
//! This module is pure: input is one `Model` plus the app `Schema`, output
//! is one `LibraryClass`. No side-effects, no per-target choices. Per-Rails-
//! idiom lowering is a separate function so each can be tested in
//! isolation (skeleton, schema columns, has_many, belongs_to, validates,
//! callbacks, …).
//!
//! Strangler-fig direction: this lives alongside the existing per-target
//! emit paths. Callers that consume the post-lowering shape opt in
//! explicitly; the rich `Model` dialect remains the input for emitters
//! that haven't migrated.

use crate::dialect::{LibraryClass, MethodDef, Model};
use crate::schema::Schema;

/// Entry point: take a `Model` (Rails-shape, with DSL items in `body`) and
/// produce the post-lowering `LibraryClass` whose `methods` carry every
/// Rails idiom expanded into explicit method bodies.
///
/// `schema` supplies the column list for the model's table — needed for
/// the per-column accessors / `attributes` / `[]` / `[]=` / `update` /
/// `initialize` lowerings. Models whose table isn't in the schema (rare;
/// abstract or virtual) get only the non-schema-driven methods.
pub fn lower_model_to_library_class(model: &Model, _schema: &Schema) -> LibraryClass {
    let methods: Vec<MethodDef> = Vec::new();

    LibraryClass {
        name: model.name.clone(),
        is_module: false,
        parent: model.parent.clone(),
        includes: Vec::new(),
        methods,
    }
}
