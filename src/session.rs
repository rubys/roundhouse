//! Bootstrap facade for the analyze → post-analyze-lower sequence that
//! every emit path runs after ingest.
//!
//! The full ingest → analyze → lower → diagnose dance is spread across
//! seven entry points (the three transpile/build drivers here, plus
//! `emit_preview`, `roundhouse-check`, the MCP server, and the LSP), and
//! a `roundhouse analyze` CLI is planned that must not become copy #8.
//! But those seven do NOT share one sequence: the *ingest* wrapping is
//! genuinely per-entry-point (plain `ingest_app`, a Prism-diagnostic
//! scope, a VFS overlay for the editor, survey mode on/off) and so is
//! the diagnostic post-processing (`diagnose`, ingest-gap attribution,
//! residue merging). Four of them — `emit_preview`, `roundhouse-check`,
//! MCP, LSP — deliberately run analyze *without* the post-analyze
//! lowerings, because they consume source-shaped IR (previews, type
//! checks, hovers), so pulling them through a lowering facade would
//! change their behavior, not dedup it.
//! Concern-accessor admission during ingest does inspect an isolated
//! analyzed/lowered copy for generated ownership. It does not lower the
//! returned App; source consumers still analyze its original shape.
//!
//! What the three emit-bound drivers (`roundhouse` transpile,
//! `dump_ir`, `project::build_site`) genuinely share is the step right
//! after ingest: build the analyzer, run it, then apply the shared
//! post-analyze lowerings against its class registry to reach the
//! emit-ready IR. That single seam lives here so the planned CLI has one
//! obvious function to call and the three drivers can't reconstruct it
//! subtly differently. Ingest and diagnostic handling stay at the call
//! sites, where their variation actually is.

use crate::app::App;
use crate::diagnostic::Diagnostic;

/// Analyze `app` in place and apply the shared post-analyze lowerings,
/// leaving it in the emit-ready IR shape the transpile driver, the site
/// build, and the IR dump all consume. Returns the post-analyze residue
/// diagnostics (sites a pass had to leave dynamic, with the reason);
/// callers that have no diagnostic surface — `project::build_site` —
/// discard them.
///
/// This is analyze **plus** [`crate::lower::apply_post_analyze_lowerings`];
/// entry points that want source-shaped IR (LSP/MCP/preview/check) call
/// `Analyzer` directly and skip the lowerings on purpose.
pub fn analyze_and_lower(app: &mut App) -> Vec<Diagnostic> {
    analyze_and_lower_with_target(app, crate::build_target::BuildTarget::Ruby)
}

fn analyze_and_lower_with_target(app: &mut App, target: crate::build_target::BuildTarget) -> Vec<Diagnostic> {
    let mut analyzer = crate::analyze::Analyzer::new(app);
    analyzer.analyze(app);
    app.helper_diagnostics = Some(crate::analyze::diagnose_helper_modules(app));
    crate::timings::phase("post-analyze lowerings", || {
        crate::lower::apply_post_analyze_lowerings_for(app, analyzer.class_registry(), target)
    })
}

/// [`analyze_and_lower`] for the app one target emits: the target's own
/// pre-analysis choices first. A target that stores PostgreSQL array
/// columns as the Array Rails reads types them so
/// (`lower::array_columns`); every other target sees the ingested app.
pub fn analyze_and_lower_for(app: &mut App, target: crate::build_target::BuildTarget) -> Vec<Diagnostic> {
    if matches!(target, crate::build_target::BuildTarget::Ruby | crate::build_target::BuildTarget::Jruby | crate::build_target::BuildTarget::Spinel) {
        app.native_library_keywords = true;
        for class in &mut app.library_classes {
            for method in &mut class.methods {
                for param in &mut method.params {
                    if param.from_keyword {
                        param.keyword = true;
                        param.from_keyword = false;
                    }
                    if param.from_kwrest {
                        param.keyword = true;
                        param.rest = true;
                        param.default = None;
                        param.from_kwrest = false;
                    }
                }
            }
        }
    }
    if target.stores_array_columns() {
        crate::lower::array_columns::store_arrays(app);
    }
    if target.lowers_nested_attributes() {
        crate::ingest::nested_attributes::expand(app);
        crate::ingest::nested_attributes::expand_autosave(app);
    }
    let mut refused = if target.lowers_validates_with() {
        crate::ingest::validates_with::expand(app);
        crate::ingest::mailer_defaults::expand(app);
        crate::ingest::has_many_hold::expand(app);
        let mut found = crate::ingest::each_validator::expand(app, target.as_str());
        found.extend(crate::ingest::mailer_helpers::apply(app, target.as_str()));
        found.extend(crate::ingest::mailer_callbacks::expand(app, target.as_str()));
        found
    } else {
        let mut found = crate::ingest::each_validator::refuse(app, target.as_str());
        found.extend(crate::ingest::mailer_callbacks::refuse(app, target.as_str()));
        found
    };
    if !target.parses_numericality_input() {
        refused.extend(crate::ingest::app::refuse_numericality_allow_on_untyped(app, target.as_str()));
    }
    refused.extend(crate::lower::untyped_subclass_diagnostics(app));
    refused.extend(analyze_and_lower_with_target(app, target));
    refused
}
