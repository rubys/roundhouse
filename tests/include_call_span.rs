//! An unresolved `include`'s refusal points at the `include` call
//! itself, not at whichever argument happens to name the unresolved
//! module — load-bearing when the module sits on a continuation line.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::DiagnosticKind;
use roundhouse::ingest::ingest_app_from_tree;

/// `include(\n  ExternalGem::Mixin\n)`: the call opens on line 2, the
/// unresolved module name sits on line 3. The refusal should report
/// line 2 — the `include(` the reader needs to find — not line 3.
#[test]
fn an_unresolved_include_refusal_points_at_the_include_line_not_the_argument_line() {
    let source = "module GadgetTools\n  include(\n    ExternalGem::Mixin\n  )\n\n  def polish\n    \"shiny\"\n  end\nend\n";
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/lib/gadget_tools.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);

    let lc = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "GadgetTools")
        .expect("GadgetTools ingested");
    let refusal = lc
        .unknown_calls
        .iter()
        .find(|call| {
            matches!(&call.diagnostic,
                Some(DiagnosticKind::Unsupported { construct, detail, .. })
                if construct.as_str() == "include" && detail.contains("ExternalGem::Mixin"))
        })
        .expect("an include refusal for ExternalGem::Mixin");

    assert!(!refusal.span.is_synthetic(), "the refusal should carry a location at all");
    let source_file = &app.sources[refusal.span.file.0 as usize - 1];
    let (line, _) = source_file.line_col(refusal.span.start);
    assert_eq!(line, 2, "should point at `include(` (line 2), not the argument (line 3)");
}
