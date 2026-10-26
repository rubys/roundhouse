//! A key Rails' `NumericalityValidator` does not read is ignored, as
//! Rails ignores it, with a note naming it (and the option it most
//! likely meant). The note is not a survey gap. A key Rails reads but
//! this build does not lower (`odd:`) still refuses the call.

use roundhouse::dialect::ValidationRule;

const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :count, numericality: { greater_than_or_equal: 0 }
  validates :size, numericality: { only_integer: true, frobnicate: true }
  validates :parity, numericality: { odd: true }
  validates :fresh, numericality: { except_on: :create }
  validates :other, numericality: { attributes: [:count] }
  validates :early, numericality: { prepend: true }
end
"#;

fn ingest() -> (roundhouse::Model, Vec<roundhouse::diagnostic::Diagnostic>, usize) {
    roundhouse::ingest::survey::activate();
    let (model, diags) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_model(MODEL.as_bytes(), "app/models/widget.rb", &roundhouse::schema::Schema::default(), &Default::default())
            .unwrap()
            .unwrap()
    });
    let gaps = roundhouse::ingest::survey::drain().len();
    (model, diags, gaps)
}

fn numericality(model: &roundhouse::Model, attr: &str) -> Option<ValidationRule> {
    model.body.iter().find_map(|item| match item {
        roundhouse::dialect::ModelBodyItem::Validation { validation, .. } if validation.attribute.as_str() == attr => {
            validation.rules.iter().find(|rule| matches!(rule, ValidationRule::Numericality { .. })).cloned()
        }
        _ => None,
    })
}

#[test]
fn an_unread_option_is_ignored_with_a_note() {
    let (model, diags, gaps) = ingest();
    assert!(
        matches!(numericality(&model, "count"), Some(ValidationRule::Numericality { gte: None, gt: None, only_integer: false, .. })),
        "{:?}",
        numericality(&model, "count")
    );
    assert!(
        matches!(numericality(&model, "size"), Some(ValidationRule::Numericality { only_integer: true, .. })),
        "the rule keeps the options Rails reads"
    );
    let notes: Vec<&roundhouse::diagnostic::Diagnostic> =
        diags.iter().filter(|d| d.code() == "ignored_validator_option").collect();
    assert_eq!(notes.len(), 2, "{diags:#?}");
    assert!(notes[0].message.contains("greater_than_or_equal") && notes[0].message.contains("did you mean `greater_than_or_equal_to:`"), "{}", notes[0].message);
    assert!(notes[1].message.contains("frobnicate") && !notes[1].message.contains("did you mean"), "{}", notes[1].message);
    assert!(notes.iter().all(|d| d.severity == roundhouse::diagnostic::Severity::Warning));
    assert_eq!(numericality(&model, "parity"), None, "odd: stays a gap");
    // Rails consumes these before the validator runs (Rails 8.1.4:
    // `except_on: :create` skips a new record, `attributes: [:count]`
    // validates `count` instead), so they are gaps, not ignored keys.
    for attr in ["fresh", "other", "early"] {
        assert_eq!(numericality(&model, attr), None, "{attr} stays a gap");
    }
    assert_eq!(gaps, 4, "odd:, except_on:, attributes: and prepend: are the survey gaps");
}
