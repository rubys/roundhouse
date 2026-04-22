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

/// Flat has_many row — `(owner_class, assoc_name, target_class,
/// foreign_key)`. Shape view emitters reach for when they need to
/// resolve `owner.<assoc>` reads into a target-typed query (rust
/// + python + go + crystal all do this; TS uses a different
/// CollectionProxy model and stays on `resolve_has_many` directly).
#[derive(Clone, Debug)]
pub struct HasManyRow {
    pub owner_class: String,
    pub assoc_name: String,
    pub target_class: String,
    pub foreign_key: String,
}

/// Flatten every `has_many` declaration in the app into a lookup
/// table. Views use this to lower `article.comments` reads into
/// `Comment.all.filter(c => c.article_id == article.id)` shapes
/// without re-walking `app.models.associations()` per target.
pub fn build_has_many_table(app: &App) -> Vec<HasManyRow> {
    let mut rows = Vec::new();
    for model in &app.models {
        for a in model.associations() {
            if let crate::dialect::Association::HasMany {
                name,
                target,
                foreign_key,
                ..
            } = a
            {
                rows.push(HasManyRow {
                    owner_class: model.name.0.as_str().to_string(),
                    assoc_name: name.as_str().to_string(),
                    target_class: target.0.as_str().to_string(),
                    foreign_key: foreign_key.as_str().to_string(),
                });
            }
        }
    }
    rows
}

/// Look up a has_many row by a local name + association name,
/// using the scaffold naming convention: the local's singular-
/// camelized form matches the owner class. `article` + `comments`
/// → resolves against the `Article` has_many :comments row.
///
/// Returns `(target_class, foreign_key)` when matched — the two
/// pieces view emit needs for its inline filter.
pub fn resolve_has_many_on_local(
    table: &[HasManyRow],
    local: &str,
    assoc: &str,
) -> Option<(String, String)> {
    let owner_class = crate::naming::singularize_camelize(local);
    table
        .iter()
        .find(|row| row.owner_class == owner_class && row.assoc_name == assoc)
        .map(|row| (row.target_class.clone(), row.foreign_key.clone()))
}
