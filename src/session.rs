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
    let mut analyzer = crate::analyze::Analyzer::new(app);
    analyzer.analyze(app);
    crate::lower::apply_post_analyze_lowerings(app, analyzer.class_registry())
}
