//! The Roda target (`--target roda`, the issue #67 spike) emits from
//! the ingest-shape `App` directly — it skips `analyze_and_lower`
//! entirely (see `emit::roda`'s module doc) and so never ran
//! `lower::model_to_library::resolve_model_aliases`, the pass every
//! other target uses to decide whether a model's `alias new old`
//! names anything real. An `alias` naming nothing (`alias missing
//! not_defined`) was re-emitted into the Sequel model file verbatim —
//! valid Ruby syntax that raises `NameError` the instant the class
//! loads, with no diagnostic anywhere to explain why.
//!
//! The fix reuses `resolve_model_aliases` (via
//! `lower_model_to_library_class`) from `emit::roda::model_for` itself,
//! so an unresolved alias gets the exact same
//! `alias_target_unresolved` diagnostic every other target raises for
//! it, and a resolvable one still emits the plain `alias` Ruby idiom
//! Sequel models (ordinary Ruby classes) need.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::emit;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.integer "state", default: 0, null: false
  end
end
"#;
const APP_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

fn tree(widget_body: &str) -> HashMap<PathBuf, Vec<u8>> {
    [
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APP_RECORD),
        ("app/models/widget.rb", widget_body),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect()
}

fn widget_model_source(files: &[roundhouse::emit::EmittedFile]) -> String {
    files
        .iter()
        .find(|f| f.path.display().to_string().ends_with("widget.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("widget.rb must be emitted"))
}

/// The regression: before the fix, this verbatim-emits `alias
/// missing_thing nowhere_defined` with no diagnostic at all.
#[test]
fn an_unresolved_alias_is_refused_with_the_same_diagnostic_as_every_other_target() {
    let app = ingest_app_from_tree(tree(
        "class Widget < ApplicationRecord\n  alias missing_thing nowhere_defined\nend\n",
    ))
    .expect("ingest");

    let (files, diags) = emit::diagnostics::scope(|| emit::roda::emit(&app));

    let alias_diags: Vec<_> = diags
        .iter()
        .filter(|d| {
            matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "alias_target_unresolved")
        })
        .collect();
    assert_eq!(
        alias_diags.len(),
        1,
        "expected exactly one alias_target_unresolved diagnostic, got: {diags:?}"
    );
    assert_eq!(alias_diags[0].severity, Severity::Error);

    let model_src = widget_model_source(&files);
    assert!(
        !model_src.contains("alias missing_thing nowhere_defined"),
        "the unresolved alias must not be emitted verbatim (NameError at load): {model_src}"
    );
}

/// Resolvable aliases (naming a hand-written method, here) must still
/// work under Roda — the fix must not turn every alias into a refusal.
#[test]
fn a_resolvable_alias_still_emits_as_plain_ruby_alias() {
    let app = ingest_app_from_tree(tree(
        "class Widget < ApplicationRecord\n  def greeting\n    \"hi\"\n  end\n  alias greet greeting\nend\n",
    ))
    .expect("ingest");

    let (files, diags) = emit::diagnostics::scope(|| emit::roda::emit(&app));

    let alias_diags: Vec<_> = diags
        .iter()
        .filter(|d| {
            matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "alias_target_unresolved")
        })
        .collect();
    assert!(alias_diags.is_empty(), "a resolvable alias must not be refused: {diags:?}");

    let model_src = widget_model_source(&files);
    assert!(
        model_src.contains("alias greet greeting"),
        "the resolvable alias must still emit as plain Ruby `alias`: {model_src}"
    );
}

/// Same enum-predicate shape `model_file_nested_class_scan.rs` covers
/// for the main ingest path — the enum method is synthesized at
/// ingest time (before Roda's resolver runs), so it must resolve too.
#[test]
fn an_alias_of_an_enum_predicate_still_resolves_under_roda() {
    let app = ingest_app_from_tree(tree(
        "class Widget < ApplicationRecord\n  enum :state, { idle: 0, accepted: 1 }\n  alias enabled? accepted?\nend\n",
    ))
    .expect("ingest");

    let (files, diags) = emit::diagnostics::scope(|| emit::roda::emit(&app));
    let alias_diags: Vec<_> = diags
        .iter()
        .filter(|d| {
            matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "alias_target_unresolved")
        })
        .collect();
    assert!(alias_diags.is_empty(), "an enum-predicate alias must resolve: {diags:?}");

    let model_src = widget_model_source(&files);
    assert!(
        model_src.contains("alias enabled? accepted?"),
        "got:\n{model_src}"
    );
}
