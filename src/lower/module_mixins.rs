//! `X.prepend Y` / `X.include Y` from a `config/initializers/` file:
//! keep the ones this tree can actually perform, report the rest.
//!
//! Ingest records every mixin it recognizes without resolving either
//! constant, because at ingest time the tree does not yet have all its
//! classes. This pass runs once it does, and answers the only question
//! that matters about a mixin: **will this line resolve at boot?**
//!
//! A mixin whose target or module nothing defines must not be emitted.
//! `WebPush::Request.prepend WebPush::PersistentRequest` in a tree with
//! no `WebPush::Request` is not a partially-working override — it is a
//! `NameError` at require time that takes the whole boot down. Dropping
//! it silently is the other failure, and the worse one for this
//! particular construct: a prepend that vanishes leaves a module
//! defined, tested, and absent from the lookup chain. So an
//! unresolvable mixin is dropped AND reported.
//!
//! "Defined" is not the same question as "ingested". A mixin can target
//! a FRAMEWORK class the runtime ships — campfire's other prepend puts
//! `RoomStreamsAreAuthorized` onto `Turbo::StreamsChannel`, which is
//! turbo-rails' own — so `RUNTIME_MIXIN_TARGETS` below names the ones
//! this pipeline models, and the rule for getting on that list is a
//! test that runs the override.
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
    if app.module_mixins.is_empty() && app.initializer_filters.is_empty() {
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

    // The filters, against the mixins that survived: a filter is kept
    // when its target is a runtime controller carrying the seam and a
    // kept mixin onto that target supplies the method. Anything else
    // would be a `NoMethodError` on the first request to that route —
    // or, on an ingested controller, a filter the controller lowering
    // never saw — so it is dropped and reported at the same standing
    // as a dropped mixin.
    let filters = std::mem::take(&mut app.initializer_filters);
    let mut kept_filters = Vec::new();
    for filter in filters {
        let target = filter.target.as_str();
        let seam = RUNTIME_FILTER_SEAMS.contains(&target);
        let supplied = app
            .module_mixins
            .iter()
            .filter(|m| m.target == filter.target)
            .any(|m| module_defines(app, &m.module, &filter.method));
        if seam && supplied {
            kept_filters.push(filter);
            continue;
        }
        let why = if !seam {
            format!("{target} is not a framework controller this tree runs initializer filters on")
        } else {
            format!("no module mixed into {target} by an initializer defines `{}`", filter.method.as_str())
        };
        let mut d = Diagnostic::unsupported(
            Span::synthetic(),
            None,
            "initializer before_action",
            format!(
                "`{target}.before_action :{}` was dropped: {why}. \
                 The guard it names never runs on that controller's actions.",
                filter.method.as_str(),
            ),
        );
        d.severity = crate::diagnostic::Severity::Warning;
        diags.push(d);
    }
    app.initializer_filters = kept_filters;
    diags
}

/// Whether the ingested module `name` defines an instance method
/// `method` — itself, or through a module it includes (one level: a
/// concern including a concern is the depth campfire's guard has).
fn module_defines(app: &App, name: &Symbol, method: &Symbol) -> bool {
    let Some(lc) = app.library_classes.iter().find(|lc| lc.name.0 == *name) else { return false };
    if lc.methods.iter().any(|m| m.name == *method) {
        return true;
    }
    lc.includes.iter().any(|inc| {
        app.library_classes
            .iter()
            .find(|l| l.name == *inc)
            .is_some_and(|l| l.methods.iter().any(|m| m.name == *method))
    })
}

/// Framework classes the RUNTIME defines that a mixin may target.
///
/// NOT a copy of the runtime's constant list, and the difference is
/// what keeps this short. The question here is not "does some file
/// under `runtime/` define this name" — it is "is this a class this
/// pipeline models well enough that prepending onto it does what the
/// app meant". A mixin overrides METHODS, so the answer depends on the
/// runtime class having the method the module calls `super` on, in a
/// path something actually reaches. Every entry earns its way on by a
/// test that runs the override.
///
/// `Turbo::StreamsChannel` (runtime/spinel/turbo_streams.rb) is the
/// stock turbo-rails channel. campfire prepends
/// `RoomStreamsAreAuthorized` onto it to refuse its own `:messages`
/// streams, and `tests/overlay_cable_dispatch.rb` drives a subscribe
/// through the prepended `subscribed` to its `super`.
///
/// `ActiveStorage::DirectUploadsController` and
/// `ActiveStorage::DiskController` (runtime/spinel/active_storage_disk.rb)
/// are Active Storage's direct-upload endpoints. campfire includes
/// `ActiveStorageAuthentication` into both and adds the `before_action`
/// that answers an anonymous upload with 401; the campfire suite's
/// `active_storage_authentication_test` drives a request through the
/// included guard to the 401 on both lanes, and
/// `tests/initializer_module_mixins.rs` pins the ingest, the lowering
/// and the generated reopen.
///
/// `WebPush::Request` is the web-push gem's request — the GEM's own
/// class on the ruby family, and runtime/spinel/web_push.rb's port of it
/// on spinel. campfire prepends `WebPush::PersistentRequest` onto it to
/// pin delivery to the address `Push::Subscription` vetted; the suite's
/// `web_push_persistent_request_test` asserts the socket opens to that
/// address and never to the endpoint's host, through the prepended
/// `perform`.
const RUNTIME_MIXIN_TARGETS: [&str; 4] = [
    "Turbo::StreamsChannel",
    "ActiveStorage::DirectUploadsController",
    "ActiveStorage::DiskController",
    "WebPush::Request",
];

/// The runtime controllers whose `process_action` asks
/// `initializer_filters(action_name)` before the action — the seam an
/// initializer's `before_action` is written into (`project::
/// apply_module_mixins`). A mixin target without the seam can take the
/// module but not the filter, so this list is separate.
const RUNTIME_FILTER_SEAMS: [&str; 2] =
    ["ActiveStorage::DirectUploadsController", "ActiveStorage::DiskController"];

/// Will this constant be defined when boot.rb reaches the mixin line?
///
/// Models, library classes (which is where an ingested
/// `app/channels/concerns/` module lands) and controllers are the three
/// homes an INGESTED constant has. The runtime is not in `App` at all,
/// so a runtime-provided target is credited from the short list above
/// rather than by scanning a tree that does not exist yet.
fn tree_defines(app: &App, name: &Symbol) -> bool {
    let n = name.as_str();
    RUNTIME_MIXIN_TARGETS.contains(&n)
        || app.models.iter().any(|m| m.name.0.as_str() == n)
        || app.library_classes.iter().any(|lc| lc.name.0.as_str() == n)
        || app.controllers.iter().any(|c| c.name.0.as_str() == n)
}
