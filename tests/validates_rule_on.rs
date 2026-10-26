//! `on:` inside a validator's own hash lowers as the call-level one does:
//! `:create` / `:update` / both, combined with the rule's `if:`, and
//! winning over a call-level `on:`. A custom context (`on: :publish`, run
//! only by `valid?(:publish)`) stays the reported gap, as it does at the
//! call level.

const MODEL: &str = r#"class Item < ApplicationRecord
  validates :pictures, presence: { if: :available?, on: :update }
  validates :name, presence: { on: :create }, length: { maximum: 3 }
  validates :code, presence: { on: :update }, on: :create
  validates :draft, presence: { on: :publish }
end
"#;

#[test]
fn a_rule_level_on_lowers_and_a_custom_context_stays_a_gap() {
    roundhouse::ingest::survey::activate();
    let (model, diags) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_model(MODEL.as_bytes(), "app/models/item.rb", &roundhouse::schema::Schema::default(), &Default::default())
            .unwrap()
            .unwrap()
    });
    let _ = roundhouse::ingest::survey::drain();
    let warnings: Vec<String> = diags
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Warning)
        .map(|d| d.message)
        .collect();
    let guarded = |attr: &str| {
        model.body.iter().any(|item| matches!(item,
            roundhouse::dialect::ModelBodyItem::Validation { validation, .. }
                if validation.attribute.as_str() == attr && !validation.rules.is_empty() && validation.if_cond.is_some()))
    };
    for attr in ["pictures", "name", "code"] {
        assert!(guarded(attr), "{attr} lowers with a guard: {warnings:?}");
        assert!(!warnings.iter().any(|w| w.contains(&format!("validates :{attr},"))), "{attr}: {warnings:?}");
    }
    assert!(!guarded("draft"), "a custom context does not lower");
    assert!(warnings.iter().any(|w| w.contains("validates :draft, on:")), "{warnings:?}");
}
