//! `X.prepend Y` / `X.include Y` from a `config/initializers/` file:
//! keep the ones this tree can actually perform, report the rest.
//!
//! Ingest records every mixin it recognizes without resolving either
//! constant, because at ingest time the tree does not yet have all its
//! classes. This pass runs once it does, and answers the only question
//! that matters about a mixin: **will this line resolve at boot?**
//!
//! A mixin whose target or module the tree does not define must not be
//! emitted. `Turbo::StreamsChannel.prepend RoomStreamsAreAuthorized` in
//! a tree with no `Turbo::StreamsChannel` is not a partially-working
//! guard — it is a `NameError` at require time that takes the whole
//! boot down. Dropping it silently is the other failure, and the worse
//! one for this particular construct: a prepend that vanishes leaves an
//! authorization module defined, tested, and absent from the lookup
//! chain. So an unresolvable mixin is dropped AND reported.
//!
//! Reported as `unsupported` rather than `lower_residue`: residue is
//! app code this pipeline chose not to lower, and this is app code it
//! WOULD lower if a class it names existed. The gap is the missing
//! class, and the diagnostic names it.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::ident::Symbol;
use crate::span::Span;

/// Drop the mixins this tree cannot perform, reporting each one.
pub fn apply_module_mixins_lowering(app: &mut App) -> Vec<Diagnostic> {
    if app.module_mixins.is_empty() {
        return Vec::new();
    }

    let mut diags = Vec::new();
    let mixins = std::mem::take(&mut app.module_mixins);
    let mut kept = Vec::new();

    for mixin in mixins {
        let target_known = tree_defines(app, &mixin.target);
        let module_known = tree_defines(app, &mixin.module);
        if target_known && module_known {
            kept.push(mixin);
            continue;
        }
        // Name the half that is missing. "Turbo::StreamsChannel is not
        // defined in this tree" is a modeling gap someone can close;
        // "the mixin was dropped" is not.
        // The whole clause, not a fragment: "neither X nor Y" carries
        // its own negation and the singular forms do not, so building
        // one sentence out of two halves loses the `not` in one arm.
        let missing = match (target_known, module_known) {
            (false, true) => format!("{} is not defined in this tree", mixin.target.as_str()),
            (true, false) => format!("{} is not defined in this tree", mixin.module.as_str()),
            _ => format!(
                "neither {} nor {} is defined in this tree",
                mixin.target.as_str(),
                mixin.module.as_str()
            ),
        };
        let mut d = Diagnostic::unsupported(
            Span::synthetic(),
            None,
            "initializer module mixin",
            format!(
                "`{}.{} {}` was dropped: {missing}. \
                 The module stays defined and OUT of the lookup chain, so anything \
                 it overrides runs unguarded.",
                mixin.target.as_str(),
                mixin.kind.as_str(),
                mixin.module.as_str(),
            ),
        );
        // WARNING, not the kind default. An unperformed mixin is a gap
        // in what this tree models, not a reason to refuse to emit one:
        // campfire prepends onto `WebPush::Request`, a GEM class no tree
        // here will ever define, and at Error severity that one line
        // would block the emit forever. Same standing as the
        // `warning[unsupported]` entries the emit already carries — it
        // belongs in the modeling-debt ledger, and the ledger is read,
        // not enforced.
        d.severity = crate::diagnostic::Severity::Warning;
        diags.push(d);
    }

    app.module_mixins = kept;
    diags
}

/// Does the emitted tree define this constant?
///
/// Models, library classes (which is where an ingested
/// `app/channels/concerns/` module lands) and controllers are the three
/// homes an ingested constant has. A runtime-provided class is NOT
/// counted: the runtime is not in `App`, and crediting it from a list
/// here would be a second copy of that list to keep in step — the same
/// trap `apply_test_gem_wiring` avoids by reading the emitted tree.
fn tree_defines(app: &App, name: &Symbol) -> bool {
    let n = name.as_str();
    app.models.iter().any(|m| m.name.0.as_str() == n)
        || app.library_classes.iter().any(|lc| lc.name.0.as_str() == n)
        || app.controllers.iter().any(|c| c.name.0.as_str() == n)
}
