//! A validation in a concern's `included do` that names a constant of
//! the concern's own module body:
//!
//! ```ruby
//! module Describable
//!   extend ActiveSupport::Concern
//!   DESCRIPTION_LIMIT = 150
//!   included do
//!     validates :description, length: { maximum: DESCRIPTION_LIMIT }
//!   end
//! end
//! ```
//!
//! The block keeps its lexical scope when Rails runs it in the
//! includer, so the constant resolves from the concern (`150`), even
//! when the includer defines one of the same name, and an
//! includer-only constant is a NameError there. The `included do`
//! statements were read with no constants at all, so the bound was
//! the reported "length: maximum:" gap. They now read the concern's
//! own (`ClassConsts::collect` over its module body), under the same
//! folding rules as a class body's.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::ValidationRule;
use roundhouse::ingest::{ingest_app_from_tree, survey};

const CONCERN: &str = r#"module Describable
  extend ActiveSupport::Concern

  DESCRIPTION_LIMIT = 150
  KIND_RE = /\A[a-z]+\z/

  included do
    validates :description, length: { maximum: DESCRIPTION_LIMIT }
    validates :kind, format: { with: KIND_RE }
  end
end
"#;

/// The includer defines its own `DESCRIPTION_LIMIT`, which Rails never
/// reads from the concern's block.
const MODEL: &str = r#"class Widget < ApplicationRecord
  DESCRIPTION_LIMIT = 99
  include Describable
end
"#;

fn ingest() -> (roundhouse::App, Vec<String>) {
    let ((app, gaps), _) = ingest_with_warnings();
    (app, gaps)
}

/// The ingest, plus the warnings it reports (what the CLI prints).
fn ingest_with_warnings() -> ((roundhouse::App, Vec<String>), Vec<String>) {
    let (result, diags) = roundhouse::ingest::prism::scope(ingest_inner);
    let warnings = diags
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Warning)
        .map(|d| d.message)
        .collect();
    (result, warnings)
}

fn ingest_inner() -> (roundhouse::App, Vec<String>) {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"description\"\n    t.string \"kind\"\n    t.string \"title\"\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/concerns/describable.rb", CONCERN),
        ("app/models/widget.rb", MODEL),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let gaps = survey::drain().into_iter().map(|g| g.to_string()).collect();
    (app, gaps)
}

fn rules<'a>(app: &'a roundhouse::App, attr: &str) -> Vec<&'a ValidationRule> {
    let widget = app.models.iter().find(|m| m.name.0.as_str() == "Widget").expect("Widget");
    widget
        .body
        .iter()
        .filter_map(|item| match item {
            roundhouse::dialect::ModelBodyItem::Validation { validation, .. } if validation.attribute.as_str() == attr => {
                Some(&validation.rules)
            }
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn a_concern_constant_folds_into_its_included_validation() {
    let (app, gaps) = ingest();
    assert!(
        matches!(rules(&app, "description").as_slice(), [ValidationRule::Length { max: Some(150), .. }]),
        "the concern's 150, not the includer's 99: {:?} {gaps:?}",
        rules(&app, "description")
    );
    assert!(
        matches!(rules(&app, "kind").as_slice(), [ValidationRule::Format { .. }]),
        "{:?} {gaps:?}",
        rules(&app, "kind")
    );
    assert!(!gaps.iter().any(|g| g.contains(":description") || g.contains(":kind")), "{gaps:?}");
}

/// A validation that lowers is not also reported as a gap. The
/// per-includer accessor probe re-reads each `included do` statement and
/// discards what it builds; the warnings that re-read produced were
/// printed anyway, so the CLI said "length: maximum: not supported" for
/// a check the includer runs.
#[test]
fn a_lowered_concern_validation_reports_no_warning() {
    let (_, warnings) = ingest_with_warnings();
    assert!(
        !warnings.iter().any(|w| w.contains("validates :description") || w.contains("validates :kind")),
        "{warnings:?}"
    );
}
