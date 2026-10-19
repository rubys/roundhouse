//! The contract every Fractal transformation is written against.
//!
//! A pass looks at one admitted [`Region`] (a method in a selected class)
//! and may propose a variant of it: edits to spans inside the method's
//! definition, plus the assumptions the edited method relies on. The
//! driver, not the pass, turns a proposal into an installed method, so
//! every pass gets the same guarantees:
//!
//! - the app's own source is never edited; a variant is a second
//!   definition of the method, installed when its owner loads and only
//!   after every guard holds, so a failed guard leaves Rails running the
//!   app's code;
//! - the variant keeps the method's name, owner, visibility, lexical
//!   nesting, file, line and `frozen_string_literal` setting;
//! - edits outside the definition, or overlapping each other, are
//!   refused with the pass's name and the reason.
//!
//! What a pass must still guarantee itself: that the edited body does
//! what the original does, observably, for every caller. Application
//! knowledge (the analyzer's types and effects in [`Region::analyzed`])
//! is what lets a pass show that; a pass that cannot should decline.

use serde::Serialize;

use crate::App;
use crate::span::Span;

use super::region::Region;

/// One compile-time transformation.
pub trait Pass {
    /// Stable, short, kebab-case: it appears in reports and variant keys.
    fn name(&self) -> &'static str;

    /// Propose a variant of `region`, or decline with a reason. Called
    /// only for admitted regions.
    fn run(&self, cx: &Cx<'_>, region: &Region<'_>) -> Proposal;
}

/// What a pass can read besides the region.
pub struct Cx<'a> {
    pub app: &'a App,
}

/// A pass's answer for one region.
#[derive(Default)]
pub struct Proposal {
    pub edits: Vec<Edit>,
    /// Assumptions the variant needs beyond the ones the driver adds
    /// for every variant (see [`Guard`]).
    pub guards: Vec<Guard>,
    /// Sites the pass looked at and left alone, with why.
    pub declined: Vec<(Span, String)>,
    /// The pass vouches that running the variant twice, or the original
    /// and the variant one after the other, is unobservable, so the
    /// runtime's verify mode may compare them. The driver also requires
    /// the analyzer to agree (`Knowledge::replayable`).
    pub replayable: bool,
}

/// Replace the source text at `span` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub text: String,
}

/// An assumption the runtime checks when the owner loads, before it
/// installs the variant. The driver adds two to every variant: the
/// method being replaced is still the app's own definition at its file
/// and line, and the file is byte for byte the one the build read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Guard {
    /// `constant`'s method `method` (its singleton method when
    /// `singleton`) is still the definition at `file`:`line`, relative to
    /// the app root. What a variant that relies on another app method's
    /// body (by inlining it, say) has to know at install time.
    DefinedAt { constant: String, method: String, singleton: bool, file: String, line: u32 },
}

/// The passes a build runs, in order. None yet: this is the baseline the
/// first ones attach to.
pub fn registered() -> Vec<Box<dyn Pass>> {
    Vec::new()
}
