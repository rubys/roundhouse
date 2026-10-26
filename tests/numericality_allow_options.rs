//! `numericality:` with `allow_nil:` / `allow_blank:` lowers to a rule
//! instead of refusing the whole `validates` call. The flags come from
//! the validator's own hash or from the call, and the hash wins.

use roundhouse::dialect::ValidationRule;

const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :min_favs, numericality: { greater_than_or_equal_to: 1, allow_nil: true }
  validates :age, numericality: { only_integer: true }, allow_blank: true
  validates :rank, numericality: { allow_nil: false }, allow_nil: true
  validates :level, numericality: true, allow_nil: true
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

fn numericality<'a>(model: &'a roundhouse::Model, attr: &str) -> Option<&'a ValidationRule> {
    model.body.iter().find_map(|item| match item {
        roundhouse::dialect::ModelBodyItem::Validation { validation, .. } if validation.attribute.as_str() == attr => {
            validation.rules.iter().find(|rule| matches!(rule, ValidationRule::Numericality { .. }))
        }
        _ => None,
    })
}

#[test]
fn allow_nil_and_allow_blank_lower_to_a_numericality_rule() {
    let (model, warnings) = ingest();
    assert!(warnings.is_empty(), "{warnings:?}");
    for attr in ["min_favs", "age", "rank", "level"] {
        assert!(numericality(&model, attr).is_some(), "{attr} lowers: {:?}", model.body);
    }
}

/// On an attribute the model does not type as a number (an `attr_accessor`
/// of an `ActiveModel::Model` form, fed request Strings), `numericality:`
/// with `allow_nil:` / `allow_blank:` is decided per target. The ruby family
/// parses the String as Rails does (`tests/numericality_untyped.rs`), so it
/// keeps the rule. A strict target's check does not parse it (#1158), so
/// there it stays the reported gap, and the whole call goes. The same
/// options on an integer column lower everywhere.
#[test]
fn on_an_untyped_attribute_the_options_are_a_gap_only_where_unparsed() {
    use std::collections::HashMap;
    use std::path::PathBuf;
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"policies\", force: :cascade do |t|\n    t.integer \"age\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/policy.rb", "class Policy < ApplicationRecord\n  validates :age, numericality: { only_integer: true }, allow_blank: true\nend\n"),
        (
            "app/models/settings_form.rb",
            "class SettingsForm\n  include ActiveModel::Model\n  attr_accessor :age, :limit, :count, :level\n  validates :age, numericality: { only_integer: true }, allow_blank: true\n  validates :limit, numericality: { allow_nil: true }\n  validates :count, presence: true, numericality: { allow_nil: true }\n  validates :level, presence: { if: :ready? }, numericality: { allow_nil: true }\n  def ready? = true\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    roundhouse::ingest::survey::activate();
    let ingested = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    assert!(roundhouse::ingest::survey::drain().iter().all(|g| !g.to_string().contains("#1158")), "ingest no longer refuses");
    let has_validation = |app: &roundhouse::App, model: &str, attr: &str| {
        app.models.iter().find(|m| m.name.0.as_str() == model).is_some_and(|m| {
            m.body.iter().any(|item| matches!(item,
                roundhouse::dialect::ModelBodyItem::Validation { validation, .. } if validation.attribute.as_str() == attr))
        })
    };
    let has_numericality = |app: &roundhouse::App, model: &str, attr: &str| {
        app.models.iter().find(|m| m.name.0.as_str() == model).is_some_and(|m| {
            m.body.iter().any(|item| matches!(item,
                roundhouse::dialect::ModelBodyItem::Validation { validation, .. }
                    if validation.attribute.as_str() == attr
                        && validation.rules.iter().any(|r| matches!(r, ValidationRule::Numericality { .. }))))
        })
    };

    // The ruby family keeps every rule.
    let mut ruby = ingested.clone();
    let ruby_diags = roundhouse::session::analyze_and_lower_for(&mut ruby, roundhouse::project::BuildTarget::Ruby);
    assert!(!ruby_diags.iter().any(|d| d.message.contains("#1158")), "{ruby_diags:?}");
    for attr in ["age", "limit", "count", "level"] {
        assert!(has_numericality(&ruby, "SettingsForm", attr), "{attr} kept on ruby");
    }

    // A strict target refuses the untyped lines, and keeps the column's.
    let mut rust = ingested.clone();
    let rust_diags: Vec<String> = roundhouse::session::analyze_and_lower_for(&mut rust, roundhouse::project::BuildTarget::Rust)
        .into_iter()
        .map(|d| d.message)
        .collect();
    assert!(has_numericality(&rust, "Policy", "age"), "an integer column lowers: {rust_diags:?}");
    for gap in ["validates :age, numericality: allow_blank:", "validates :limit, numericality: allow_nil:"] {
        assert!(rust_diags.iter().any(|g| g.contains(gap) && g.contains("(rust)") && g.contains("#1158")), "{gap}: {rust_diags:?}");
    }
    // The whole call goes, as an ingest refusal does: `presence:` beside
    // the numericality is not kept, nor is the item a per-rule `if:` split
    // off the same call.
    for attr in ["age", "limit", "count", "level"] {
        assert!(!has_validation(&rust, "SettingsForm", attr), "no validation left for {attr} on rust");
    }
}
