//! Numericality bounds written as Integer arithmetic: literals and the
//! model's own Integer constants combined with `+`, `-`, `*`, `**`, unary
//! minus and parentheses, plus the `in:` range form. Anything else (a
//! division, a method call, an operand that does not fold, a result too
//! large to compare exactly) stays the reported gap.

const MODEL: &str = r#"class Role < ApplicationRecord
  POSITION_LIMIT = (2**31) - 1
  SIGN_COUNT_LIMIT = (2**63)
  LEVEL_MAX = 4 * (2 + 3)
  HALF = 7 / 2
  COMPUTED = compute_limit * 2
  HUGE = 2**200

  validates :position, numericality: { in: (-POSITION_LIMIT..POSITION_LIMIT) }
  validates :sign_count, numericality: { only_integer: true, greater_than_or_equal_to: 0, less_than: SIGN_COUNT_LIMIT }
  validates :level, numericality: { less_than_or_equal_to: LEVEL_MAX - 1 }
  validates :tier, numericality: { in: 1...3 }
  validates :half, numericality: { less_than: HALF }
  validates :computed, numericality: { less_than: COMPUTED }
  validates :huge, numericality: { less_than: HUGE }
end
"#;

fn ingest() -> (roundhouse::Model, Vec<String>) {
    roundhouse::ingest::survey::activate();
    let (model, diags) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_model(MODEL.as_bytes(), "app/models/role.rb", &roundhouse::schema::Schema::default(), &Default::default())
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

fn lowered(model: &roundhouse::Model, attr: &str) -> bool {
    model.body.iter().any(|item| matches!(item,
        roundhouse::dialect::ModelBodyItem::Validation { validation, .. }
            if validation.attribute.as_str() == attr
                && validation.rules.iter().any(|r| matches!(r, roundhouse::dialect::ValidationRule::Numericality { .. }))))
}

#[test]
fn integer_arithmetic_and_in_ranges_lower() {
    let (model, warnings) = ingest();
    for attr in ["position", "sign_count", "level", "tier"] {
        assert!(lowered(&model, attr), "{attr} lowers: {warnings:?}");
        assert!(!warnings.iter().any(|w| w.contains(&format!("validates :{attr},"))), "{attr}: {warnings:?}");
    }
}

#[test]
fn other_expressions_stay_the_gap() {
    let (model, warnings) = ingest();
    for attr in ["half", "computed", "huge"] {
        assert!(!lowered(&model, attr), "{attr} stays a gap");
        assert!(warnings.iter().any(|w| w.contains(&format!("validates :{attr}, numericality: less_than:"))), "{attr}: {warnings:?}");
    }
}
