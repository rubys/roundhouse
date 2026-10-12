//! `primary_key:` on associations: one place that says which paths
//! honor it and which refuse.
//!
//! Rails' `has_one` / `has_many` take `primary_key:` naming the OWNER
//! column the foreign key matches (`has_one :source_profile,
//! foreign_key: :source_id, primary_key: :source_id` is
//! `SourceProfile.where(source_id: article.source_id)`, not
//! `article.id`). A `belongs_to` takes it as the TARGET column.
//!
//! Ignoring the option returns the wrong rows without an error, so the
//! contract is: every consumer of the association struct either reads
//! [`Association::owner_key`] or is listed here as refused.
//!
//! `belongs_to primary_key:` (the TARGET column) is handled by the
//! belongs_to reader/writer/join code, not here.
//!
//! Honored (read `owner_key()`): the has_one/has_many reader, the
//! `has_one` autosave stamp, the batch preload behind `includes` /
//! `preload`, the `joins` ON clause, `build` / `create` through a
//! has_many (the seeded foreign key) and `build_x` / `create_x` on a
//! has_one. `dependent: :destroy` cascades through the reader, so it
//! follows.
//!
//! Refused with an `unsupported` error (see [`refuse`]):
//!   * `through:` combined with `primary_key:`, and a `through:` chain
//!     whose hop (the through association or the join model's source)
//!     declares one — the chain's join SQL names `id` on each hop.
//!   * a keyed `has_one … through:` (ingest carries the `through:`, but
//!     the reader is the direct query), and a `through:` chain too deep
//!     to prove free of a keyed hop.
//!   * `reset_counters` over such a has_many, and the Futamura
//!     specializer's has_many root (both keep their own refusals).

use crate::dialect::{Association, Model};
use crate::ident::Symbol;
use crate::span::Span;

/// The declared `primary_key:` when it names something other than
/// `id` (an explicit `primary_key: :id` is the default).
pub(crate) fn custom_primary_key(assoc: &Association) -> Option<&Symbol> {
    assoc.declared_primary_key().filter(|k| k.as_str() != "id")
}

const MAX_THROUGH_DEPTH: usize = 4;

/// First `through:` hop under `model.<thr_name>` (or the join model's
/// source reaching `target`) that declares a custom `primary_key:`.
fn through_chain_custom_key(
    models: &[Model],
    model: &Model,
    thr_name: &Symbol,
    target: &crate::ident::ClassId,
    depth: usize,
) -> Option<String> {
    // The same depth `resolve_through_chain` gives up at. Running out
    // is "cannot prove no hop is keyed", which is refused rather than
    // read as "none is": past this point the direct reader stays.
    if depth > MAX_THROUGH_DEPTH {
        return Some(format!(
            "a `through:` chain deeper than {MAX_THROUGH_DEPTH} hops under `{}#{thr_name}`, \
             which cannot be checked for a keyed hop",
            model.name.0
        ));
    }
    let hop = model.associations().find(|a| a.name() == thr_name)?;
    if let Some(k) = custom_primary_key(hop) {
        return Some(format!("`{}#{}` declares `primary_key: :{}`", model.name.0, thr_name, k));
    }
    let Association::HasMany { target: thr_target, through, .. } = hop else { return None };
    if let Some(inner) = through {
        if let Some(found) = through_chain_custom_key(models, model, inner, thr_target, depth + 1) {
            return Some(found);
        }
    }
    let thr_model = models.iter().find(|m| &m.name == thr_target)?;
    for a in thr_model.associations() {
        match a {
            Association::HasMany { target: t, through: None, .. }
            | Association::HasOne { target: t, .. }
                if t == target =>
            {
                if let Some(k) = custom_primary_key(a) {
                    return Some(format!(
                        "`{}#{}` declares `primary_key: :{}`",
                        thr_model.name.0,
                        a.name(),
                        k
                    ));
                }
            }
            Association::HasMany { target: t, through: Some(thru), .. } if t == target => {
                if let Some(found) =
                    through_chain_custom_key(models, thr_model, thru, target, depth + 1)
                {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Why `assoc` cannot be synthesized correctly, when its
/// `primary_key:` (or a hop's) reaches a path that cannot honor it.
/// `None` when no custom key is on its path.
pub(crate) fn unsupported_reason(
    model: &Model,
    models: &[Model],
    assoc: &Association,
) -> Option<String> {
    match assoc {
        Association::HasMany { through: Some(thr), target, primary_key, .. } => {
            if let Some(k) = primary_key.as_ref().filter(|k| k.as_str() != "id") {
                Some(format!("`primary_key: :{k}` on a `through:` association"))
            } else {
                through_chain_custom_key(models, model, thr, target, 0)
                    .map(|hop| format!("a `through:` hop keyed on a non-id column ({hop})"))
            }
        }
        // Ingest carries `through:` on a has_one, but the reader is
        // still the direct `Target.where(fk: owner_key).first`, which a
        // keyed has_one :through would silently answer wrongly.
        Association::HasOne { through: Some(_), .. } => custom_primary_key(assoc)
            .map(|k| format!("`primary_key: :{k}` on a `has_one … through:` association")),
        _ => None,
    }
}

/// Push an `unsupported` error for an association whose `primary_key:`
/// reaches a path that cannot honor it. Silent otherwise.
/// Returns true when it refused: the caller must then synthesize no
/// reader for the association (a diagnostic next to an id-keyed or
/// wrongly keyed reader would still ship the wrong rows).
pub(crate) fn refuse(model: &Model, models: &[Model], assoc: &Association, span: Span) -> bool {
    let Some(why) = unsupported_reason(model, models, assoc) else { return false };
    crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
        span,
        None,
        "association_primary_key",
        format!(
            "`{}.{}`: {why} is not supported; its reader, preload, join and write paths \
             all key on `id`, so it would silently return the wrong rows",
            model.name.0,
            assoc.name()
        ),
    ));
    true
}
