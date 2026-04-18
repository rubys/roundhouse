//! Validation lowering smoke test.
//!
//! Phase 4 pilot. Exercises `lower_validations` against the fixture
//! models to confirm the target-neutral `Check` sequence has the
//! expected shape. Emitter-side renders (TS → Juntos calls, Rust →
//! inline evaluator) are tested separately in their per-target
//! emit tests, but this file pins down the *shape* all targets see.

use roundhouse::ingest::ingest_app;
use roundhouse::lower::{lower_validations, Check};
use std::path::Path;

fn tiny_blog_post() -> roundhouse::Model {
    let app = ingest_app(Path::new("fixtures/tiny-blog")).expect("ingest");
    app.models
        .into_iter()
        .find(|m| m.name.0.as_str() == "Post")
        .expect("Post model")
}

#[test]
fn presence_rule_lowers_to_presence_check() {
    let post = tiny_blog_post();
    let lowered = lower_validations(&post);
    // tiny-blog's Post: `validates :title, presence: true`.
    assert_eq!(lowered.len(), 1);
    let lv = &lowered[0];
    assert_eq!(lv.attribute.as_str(), "title");
    assert_eq!(lv.checks.len(), 1);
    assert!(matches!(lv.checks[0], Check::Presence));
}

#[test]
fn length_rule_fans_out_into_min_and_max_checks() {
    // Build an ad-hoc model to exercise the min+max case — tiny-blog's
    // Post only has presence. Real-blog's article has `length:
    // { minimum: 10 }` but only one bound; we want both.
    use roundhouse::{
        ClassId, Model, ModelBodyItem, Row, Symbol, TableRef, Validation, ValidationRule,
    };
    let model = Model {
        name: ClassId(Symbol::from("Widget")),
        parent: None,
        table: TableRef(Symbol::from("widgets")),
        attributes: Row::closed(),
        body: vec![ModelBodyItem::Validation {
            validation: Validation {
                attribute: Symbol::from("body"),
                rules: vec![ValidationRule::Length { min: Some(5), max: Some(100) }],
            },
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    };
    let lowered = lower_validations(&model);
    assert_eq!(lowered.len(), 1);
    assert_eq!(lowered[0].checks.len(), 2, "one for min, one for max");
    assert!(matches!(lowered[0].checks[0], Check::MinLength { n: 5 }));
    assert!(matches!(lowered[0].checks[1], Check::MaxLength { n: 100 }));
}

#[test]
fn multiple_rules_on_one_attribute_stay_grouped() {
    use roundhouse::{
        ClassId, Model, ModelBodyItem, Row, Symbol, TableRef, Validation, ValidationRule,
    };
    let model = Model {
        name: ClassId(Symbol::from("Widget")),
        parent: None,
        table: TableRef(Symbol::from("widgets")),
        attributes: Row::closed(),
        body: vec![ModelBodyItem::Validation {
            validation: Validation {
                attribute: Symbol::from("title"),
                rules: vec![
                    ValidationRule::Presence,
                    ValidationRule::Length { min: Some(3), max: None },
                ],
            },
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    };
    let lowered = lower_validations(&model);
    assert_eq!(lowered.len(), 1);
    assert_eq!(lowered[0].checks.len(), 2);
    assert!(matches!(lowered[0].checks[0], Check::Presence));
    assert!(matches!(lowered[0].checks[1], Check::MinLength { n: 3 }));
}

#[test]
fn default_messages_match_rails_defaults() {
    assert_eq!(Check::Presence.default_message(), "can't be blank");
    assert_eq!(Check::Absence.default_message(), "must be blank");
    assert_eq!(
        Check::MinLength { n: 10 }.default_message(),
        "is too short (minimum is 10 characters)"
    );
    assert_eq!(
        Check::MaxLength { n: 100 }.default_message(),
        "is too long (maximum is 100 characters)"
    );
    assert_eq!(Check::OnlyInteger.default_message(), "must be an integer");
}

#[test]
fn lowered_ir_round_trips_through_json() {
    // Serde round-trip — the lowered form needs to be shareable as JSON
    // for tooling (roundhouse-ast dumps, cross-process handoffs).
    let post = tiny_blog_post();
    let lowered = lower_validations(&post);
    let json = serde_json::to_string_pretty(&lowered).unwrap();
    let back: Vec<roundhouse::lower::LoweredValidation> = serde_json::from_str(&json).unwrap();
    assert_eq!(lowered, back);
}
