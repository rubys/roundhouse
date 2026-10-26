//! A `format: { with: CONST }` regex built by interpolating String (or
//! Integer) constants of the class body:
//!
//! ```ruby
//! SEPARATORS = "_·"
//! FIRST = "x[[:word:]#{SEPARATORS}]*".freeze
//! NAME_PAT = "#{FIRST}|[[:digit:]]+".freeze
//! NAME_RE = /\A(#{NAME_PAT})\z/i
//! ```
//!
//! Ruby splices an interpolated String into a regex literal as raw
//! pattern source (`NAME_RE.source == "\\A(x[[:word:]_·]*|[[:digit:]]+)\\z"`),
//! with the literal's own flags. Only an interpolation of one other Regexp
//! constant folded, and interpolated Strings never did, so these stayed
//! the reported "format: with:" gap.

use roundhouse::dialect::ValidationRule;

const MODEL: &str = r##"class Widget < ApplicationRecord
  SEPARATORS = "_·"
  FIRST = "x[[:word:]#{SEPARATORS}]*".freeze
  NAME_PAT = "#{FIRST}|[[:digit:]]+".freeze
  NAME_RE = /\A(#{NAME_PAT})\z/i
  CODE_FRAGMENT = '[a-z0-9_]{2,}'
  CODE_RE = /\A#{CODE_FRAGMENT}\z/
  SIZE = 3
  SIZED_RE = /\A[a-z]{#{SIZE}}\z/
  UNKNOWN_RE = /\A#{compute_fragment}\z/

  validates :name, format: { with: NAME_RE }
  validates :code, format: { with: CODE_RE }
  validates :sized, format: { with: SIZED_RE }
  validates :other, format: { with: UNKNOWN_RE }
end
"##;

fn ingest() -> (roundhouse::Model, Vec<String>) {
    roundhouse::ingest::survey::activate();
    let (model, diags) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_model(MODEL.as_bytes(), "app/models/widget.rb", &roundhouse::schema::Schema::default(), &Default::default())
            .unwrap()
            .unwrap()
    });
    let _ = roundhouse::ingest::survey::drain();
    let warnings = diags
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Warning)
        .map(|d| d.message)
        .collect();
    (model, warnings)
}

fn format_of<'a>(model: &'a roundhouse::Model, attr: &str) -> Option<(&'a str, &'a str)> {
    model.body.iter().find_map(|item| match item {
        roundhouse::dialect::ModelBodyItem::Validation { validation, .. } if validation.attribute.as_str() == attr => {
            validation.rules.iter().find_map(|rule| match rule {
                ValidationRule::Format { pattern, flags, .. } => Some((pattern.as_str(), flags.as_str())),
                _ => None,
            })
        }
        _ => None,
    })
}

/// The folded sources are Ruby's `Regexp#source` for the same constants.
#[test]
fn a_regex_built_from_string_constants_folds_to_rubys_source() {
    let (model, warnings) = ingest();
    assert_eq!(format_of(&model, "name"), Some((r"\A(x[[:word:]_·]*|[[:digit:]]+)\z", "i")), "{warnings:?}");
    assert_eq!(format_of(&model, "code"), Some((r"\A[a-z0-9_]{2,}\z", "")), "{warnings:?}");
    assert_eq!(format_of(&model, "sized"), Some((r"\A[a-z]{3}\z", "")), "{warnings:?}");
}

/// A piece the class body can't fold leaves the whole regex unfolded:
/// the validation stays the reported gap.
#[test]
fn an_unfoldable_piece_keeps_the_gap() {
    let (model, warnings) = ingest();
    assert_eq!(format_of(&model, "other"), None);
    assert!(warnings.iter().any(|w| w.contains("validates :other, format: with:")), "{warnings:?}");
}
