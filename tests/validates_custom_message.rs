//! A literal String `message:` in a validator's own hash. Rails uses it in
//! place of the default message (after a model-specific translation, if
//! the app defines one), interpolating `%{value}`, `%{attribute}`,
//! `%{model}` and, where the error supplies one, `%{count}`. It was a
//! reported gap ("format: message: not supported") that refused the call.
//!
//! A Symbol message (an i18n key), a Proc, and a String that names an
//! interpolation the rule cannot always supply (`%{count}` on
//! numericality, whose "is not a number" error has no count: Rails raises
//! `I18n::MissingInterpolationArgument`; an unknown `%{name}`) stay the
//! gap. `%%` is I18n's literal `%`, so `%%{shape}` names nothing and
//! lowers. A call-level `message:` is not a Rails option at all (`validates`
//! raises "Unknown validator: 'MessageValidator'").

const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :code, format: { with: /\A[A-Z0-9]+\z/, message: "is not a valid code (%{value})" }
  validates :name, presence: { message: "must be given" }, length: { maximum: 3 }
  validates :kind, inclusion: { in: %w[a b], message: "%{value} is not a kind" }
  validates :score, numericality: { greater_than: 1, message: "must exceed the minimum" }
  validates :title, exclusion: { in: %w[admin], message: "is reserved for %{model}" }
  validates :slug, uniqueness: { message: "is used by %{attribute}" }
  validates :escaped, presence: { message: "100%% needed, not %%{shape}" }
  validates :bounded, presence: { message: "must be %< 10" }

  validates :keyed, presence: { message: :custom_key }
  validates :called, presence: { message: -> { "computed" } }
  validates :counted, numericality: { greater_than: 1, message: "must exceed %{count}" }
  validates :unknown, format: { with: /\Ax\z/, message: "needs %{shape}" }
  validates :formatted, presence: { message: "needs %<count>d" }
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

fn lowered(model: &roundhouse::Model, attr: &str) -> bool {
    model.body.iter().any(|item| matches!(item,
        roundhouse::dialect::ModelBodyItem::Validation { validation, .. }
            if validation.attribute.as_str() == attr && !validation.rules.is_empty()))
}

#[test]
fn a_string_message_lowers_on_every_rule_kind() {
    let (model, warnings) = ingest();
    for attr in ["code", "name", "kind", "score", "title", "slug", "escaped", "bounded"] {
        assert!(lowered(&model, attr), "{attr} lowers: {warnings:?}");
        assert!(!warnings.iter().any(|w| w.contains(&format!("validates :{attr},"))), "{attr}: {warnings:?}");
    }
}

#[test]
fn other_messages_stay_the_gap() {
    let (model, warnings) = ingest();
    for attr in ["keyed", "called", "counted", "unknown", "formatted"] {
        assert!(!lowered(&model, attr), "{attr} stays a gap");
        assert!(warnings.iter().any(|w| w.contains(&format!("validates :{attr},")) && w.contains("message")), "{attr}: {warnings:?}");
    }
}
