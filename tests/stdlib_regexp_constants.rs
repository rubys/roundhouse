//! `validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }` — the
//! most common email check in Rails apps — names a Regexp constant of
//! Ruby's standard library rather than one the class body assigns, so it
//! stayed the reported "format: with:" gap and the whole `validates` call
//! was refused. Ingest now folds that constant (also spelled
//! `::URI::MailTo::EMAIL_REGEXP`) to Ruby's own pattern source and flags:
//!
//! ```text
//! $ ruby -ruri -e 'puts URI::MailTo::EMAIL_REGEXP.source; p URI::MailTo::EMAIL_REGEXP.options'
//! \A[a-zA-Z0-9.!\#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\z
//! 0
//! ```
//!
//! The unrooted spelling resolves lexically in Ruby, so it folds only
//! where nothing could shadow `URI`: a top-level class whose body
//! defines no `URI`. Any other qualified constant stays the gap.

use roundhouse::dialect::ValidationRule;

/// `URI::MailTo::EMAIL_REGEXP.source` on Ruby 4.0.5, verbatim.
const EMAIL_REGEXP_SOURCE: &str = r"\A[a-zA-Z0-9.!\#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\z";

const MODEL: &str = r##"class Person < ApplicationRecord
  validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }
  validates :backup_email, format: { with: ::URI::MailTo::EMAIL_REGEXP }
  validates :short_email, format: URI::MailTo::EMAIL_REGEXP
  validates :other, format: { with: URI::Something::ELSE }
  # The literal `uri/mailto.rb` assigns to EMAIL_REGEXP, written inline.
  validates :inline_email, format: { with: /\A[a-zA-Z0-9.!\#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\z/ }
end
"##;

fn ingest() -> (roundhouse::Model, Vec<String>) {
    ingest_source(MODEL, "app/models/person.rb")
}

fn ingest_source(source: &str, file: &str) -> (roundhouse::Model, Vec<String>) {
    roundhouse::ingest::survey::activate();
    let (model, diags) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_model(source.as_bytes(), file, &roundhouse::schema::Schema::default(), &Default::default())
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

#[test]
fn uri_mailto_email_regexp_folds_to_rubys_source_and_flags() {
    let (model, warnings) = ingest();
    assert_eq!(format_of(&model, "email"), Some((EMAIL_REGEXP_SOURCE, "")), "{warnings:?}");
    assert_eq!(format_of(&model, "backup_email"), Some((EMAIL_REGEXP_SOURCE, "")), "{warnings:?}");
    assert_eq!(format_of(&model, "short_email"), Some((EMAIL_REGEXP_SOURCE, "")), "{warnings:?}");
    for attr in ["email", "backup_email", "short_email"] {
        assert!(!warnings.iter().any(|w| w.contains(&format!("validates :{attr},"))), "{attr}: {warnings:?}");
    }
}

#[test]
fn an_unknown_qualified_constant_stays_the_gap() {
    let (model, warnings) = ingest();
    assert_eq!(format_of(&model, "other"), None);
    assert!(warnings.iter().any(|w| w.contains("validates :other, format: with:")), "{warnings:?}");
}

/// The constant lowers to exactly what the same regex literal written
/// inline lowers to, so every emit target treats the two identically.
#[test]
fn the_constant_is_the_same_rule_as_its_inline_literal() {
    let (model, warnings) = ingest();
    let inline = format_of(&model, "inline_email");
    assert!(inline.is_some(), "{warnings:?}");
    assert_eq!(format_of(&model, "email"), inline);
}

/// Inside `module Admin`, unrooted `URI` could be an app's `Admin::URI`:
/// the unrooted spelling stays the gap, the rooted one still folds.
#[test]
fn a_nested_model_folds_only_the_rooted_spelling() {
    let source = r##"module Admin
  class Account < ApplicationRecord
    validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }
    validates :backup_email, format: { with: ::URI::MailTo::EMAIL_REGEXP }
  end
end
"##;
    let (model, warnings) = ingest_source(source, "app/models/admin/account.rb");
    assert_eq!(format_of(&model, "email"), None);
    assert!(warnings.iter().any(|w| w.contains("validates :email, format: with:")), "{warnings:?}");
    assert_eq!(format_of(&model, "backup_email"), Some((EMAIL_REGEXP_SOURCE, "")), "{warnings:?}");
}

/// `class Admin::Account` is nested too, however it is spelled.
#[test]
fn a_compound_class_name_counts_as_nested() {
    let source = r##"class Admin::Account < ApplicationRecord
  validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }
end
"##;
    let (model, warnings) = ingest_source(source, "app/models/admin/account.rb");
    assert_eq!(format_of(&model, "email"), None);
    assert!(warnings.iter().any(|w| w.contains("validates :email, format: with:")), "{warnings:?}");
}

/// A top-level class whose own body defines `URI` shadows the
/// standard-library one for the unrooted spelling.
#[test]
fn a_class_body_defining_uri_keeps_the_unrooted_gap() {
    for definition in ["URI = Object.new", "module URI
  end", "class URI
  end"] {
        let source = format!(
            "class Account < ApplicationRecord\n  {definition}\n  validates :email, format: {{ with: URI::MailTo::EMAIL_REGEXP }}\n  validates :backup_email, format: {{ with: ::URI::MailTo::EMAIL_REGEXP }}\nend\n"
        );
        let (model, warnings) = ingest_source(&source, "app/models/account.rb");
        assert_eq!(format_of(&model, "email"), None, "{definition}");
        assert_eq!(format_of(&model, "backup_email"), Some((EMAIL_REGEXP_SOURCE, "")), "{definition}: {warnings:?}");
    }
}
