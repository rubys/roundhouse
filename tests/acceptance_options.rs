//! `validates :attr, acceptance:` lowers to a rule. `accept:` is Rails'
//! default `["1", true]` unless given as a literal or an Array of
//! literals, and `allow_nil:` comes from the validator's own hash or the
//! call (the hash wins). Anything else stays a reported gap.

use roundhouse::dialect::ValidationRule;
use roundhouse::expr::Literal;

const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :terms, acceptance: true
  validates :agreement, acceptance: { allow_nil: false, accept: [true, 'true', '1'] }, on: :create
  validates :eula, acceptance: { accept: 'yes' }, allow_nil: false
  validates :tos, acceptance: { allow_nil: true }, allow_nil: false
  validates :skipped, acceptance: false
  validates :odd, acceptance: { accept: :yes }
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

fn acceptance(model: &roundhouse::Model, attr: &str) -> Option<(Vec<Literal>, Option<bool>)> {
    model.body.iter().find_map(|item| match item {
        roundhouse::dialect::ModelBodyItem::Validation { validation, .. } if validation.attribute.as_str() == attr => {
            validation.rules.iter().find_map(|rule| match rule {
                ValidationRule::Acceptance { accept, allow_nil } => Some((accept.clone(), *allow_nil)),
                _ => None,
            })
        }
        _ => None,
    })
}

fn s(value: &str) -> Literal {
    Literal::Str { value: value.to_string() }
}

#[test]
fn acceptance_options_lower_to_an_acceptance_rule() {
    let (model, warnings) = ingest();
    let t = Literal::Bool { value: true };
    assert_eq!(acceptance(&model, "terms"), Some((vec![s("1"), t.clone()], None)), "Rails' default accept");
    assert_eq!(acceptance(&model, "agreement"), Some((vec![t, s("true"), s("1")], Some(false))));
    assert_eq!(acceptance(&model, "eula"), Some((vec![s("yes")], Some(false))), "a call-level allow_nil applies");
    assert_eq!(acceptance(&model, "tos").map(|(_, nil)| nil), Some(Some(true)), "the rule's own allow_nil wins");
    assert_eq!(acceptance(&model, "skipped"), None, "acceptance: false is no rule");
    assert_eq!(acceptance(&model, "odd"), None, "a Symbol accept value is not lowered");
    assert!(
        warnings.iter().any(|w| w.contains("validates :odd, acceptance: accept:")),
        "{warnings:?}"
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}
