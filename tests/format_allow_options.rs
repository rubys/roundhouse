//! `format:` with `allow_nil:` / `allow_blank:` lowers to a rule instead
//! of refusing the whole `validates` call. The flags come from the
//! validator's own hash or from the call, and the hash wins.

use roundhouse::dialect::ValidationRule;

const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :slug, format: { with: /\A[a-z]+\z/, allow_nil: true }
  validates :code, format: { with: /\A[A-Z]+\z/ }, allow_blank: true
  validates :tag, format: { with: /\A#\w+\z/, allow_nil: false }, allow_nil: true
  validates :name, format: { with: /\A\w+\z/, allow_blank: false }, allow_blank: true
  validates_format_of :site, with: /\Ahttps:/, allow_nil: true
  validates_format_of :host, with: /\A\w+\z/, allow_blank: true
end
"#;

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

fn format_flags(model: &roundhouse::Model, attr: &str) -> Option<(Option<bool>, Option<bool>)> {
    model.body.iter().find_map(|item| match item {
        roundhouse::dialect::ModelBodyItem::Validation { validation, .. } if validation.attribute.as_str() == attr => {
            validation.rules.iter().find_map(|rule| match rule {
                ValidationRule::Format { allow_nil, allow_blank, .. } => Some((*allow_nil, *allow_blank)),
                _ => None,
            })
        }
        _ => None,
    })
}

#[test]
fn allow_nil_and_allow_blank_lower_to_a_format_rule() {
    let (model, warnings) = ingest();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(format_flags(&model, "slug"), Some((Some(true), None)));
    assert_eq!(format_flags(&model, "code"), Some((None, Some(true))));
    assert_eq!(format_flags(&model, "tag"), Some((Some(false), None)), "the rule's own allow_nil wins");
    assert_eq!(format_flags(&model, "name"), Some((None, Some(false))), "the rule's own allow_blank wins");
    assert_eq!(format_flags(&model, "site"), Some((Some(true), None)));
    assert_eq!(format_flags(&model, "host"), Some((None, Some(true))));
}
