//! Typing-pass mode and the controller/mailer→view seed channel.
//!
//! Split out of `mod.rs` so the dirty-set / views-only work has a
//! named home instead of growing the analyzer god file further.

use std::collections::{BTreeSet, HashMap};

use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Which call-site trees `unify_params_from_call_sites` walks.
/// Production rounds skip views/tests/seeds: those trees are still
/// ingest-shaped until after the production fixpoint (wave 12).
/// Test sites are overlaid separately so later test-only rounds can
/// reuse a production+view param snapshot.
#[derive(Clone, Copy)]
pub(super) enum UnifyScope {
    Production,
    WithViews,
}

/// Which half of the app a typing pass walks.
///
/// This replaces a `type_views_and_tests: bool`, which only ever said
/// "views TOO" — so each of the three passes that stamp templates
/// also retyped the production bodies the fixpoint had just settled.
/// Campfire typed production fifteen times to converge it six.
#[derive(Clone, Copy)]
pub(super) enum TypingMode<'a> {
    /// Models, controllers, library classes. `dirty` restricts which
    /// of their bodies are retyped (`None` is every one of them); see
    /// [`super::Analyzer::type_production_bodies`].
    Production {
        dirty: Option<&'a std::collections::HashSet<ClassId>>,
    },
    /// Views, partials, the original test scopes, and `db/seeds.rb`,
    /// against the channel the last production pass harvested.
    ViewsAndTests,
}

/// The controller/mailer→view channel: what a views pass needs in
/// order to seed a template's `Ctx` without re-walking the controller
/// bodies that produced it. Every production pass leaves its harvest
/// here, so a views pass reads the seeds of the bodies as they
/// converged rather than rebuilding them from an identical walk.
#[derive(Clone, Default)]
pub(super) struct ViewSeeds {
    pub action_ivars_by_view: HashMap<Symbol, HashMap<Symbol, Ty>>,
    pub layout_ivars_by_view: HashMap<Symbol, HashMap<Symbol, Ty>>,
    pub content_partial_ivars: HashMap<Symbol, HashMap<Symbol, Ty>>,
    pub mailer_params_by_view: HashMap<Symbol, Ty>,
    pub view_feeders: HashMap<Symbol, BTreeSet<ClassId>>,
    pub controller_resolutions: HashMap<ClassId, crate::app::ControllerResolution>,
}
