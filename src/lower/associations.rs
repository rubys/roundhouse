//! Target-neutral association resolution.
//!
//! When a target emitter sees `owner.<assoc>.<op>` and needs to know
//! which class and foreign key the `<assoc>` reference resolves to,
//! it asks this module. The lookup is pure IR — same answer in every
//! target — so the six per-target emitters share the dispatch
//! instead of re-implementing it.
//!
//! Today: has_many only. has_one and HABTM land when a fixture
//! exercises them.

use crate::dialect::Association;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;
use crate::App;

/// Resolution of a Ruby `owner.<has_many_name>` reference.
#[derive(Clone, Debug, PartialEq)]
pub struct HasManyRef {
    /// The owner class this association is defined on. Present when
    /// resolvable via `owner_ty` (a `Ty::Class`); `None` when the
    /// lookup fell back to a cross-model scan by association name.
    pub owner_class: Option<ClassId>,
    /// Target class the association points at (`Comment` for
    /// `Article.has_many :comments`).
    pub target_class: ClassId,
    /// Foreign-key column on the target's table (`article_id`).
    pub foreign_key: Symbol,
}

/// Resolve a has_many reference. Prefers the owner's static type when
/// available; otherwise falls back to a cross-model scan (fine while
/// association names are unique across the app, which holds for
/// real-blog today).
pub fn resolve_has_many(
    assoc_name: &Symbol,
    owner_ty: Option<&Ty>,
    app: &App,
) -> Option<HasManyRef> {
    if let Some(Ty::Class { id, .. }) = owner_ty {
        if let Some(model) = app.models.iter().find(|m| m.name.0 == id.0) {
            for a in model.associations() {
                if let Association::HasMany {
                    name,
                    target,
                    foreign_key,
                    ..
                } = a
                {
                    if name == assoc_name {
                        return Some(HasManyRef {
                            owner_class: Some(model.name.clone()),
                            target_class: target.clone(),
                            foreign_key: foreign_key.clone(),
                        });
                    }
                }
            }
            return None;
        }
    }
    // Untyped owner — scan every model for a has_many with this
    // name. Unambiguous when association names are unique app-wide;
    // returns the first hit otherwise.
    for model in &app.models {
        for a in model.associations() {
            if let Association::HasMany {
                name,
                target,
                foreign_key,
                ..
            } = a
            {
                if name == assoc_name {
                    return Some(HasManyRef {
                        owner_class: Some(model.name.clone()),
                        target_class: target.clone(),
                        foreign_key: foreign_key.clone(),
                    });
                }
            }
        }
    }
    None
}
