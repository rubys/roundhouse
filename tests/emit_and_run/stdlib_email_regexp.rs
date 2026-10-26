use super::emit_and_run;

/// `validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }` (see
/// tests/stdlib_regexp_constants.rs). Answers are Rails 8.1.4's for the
/// same model and values: `a@b` is valid (the domain part needs no dot),
/// and the empty string is invalid (the validator matches `value.to_s`).
#[test]
fn uri_mailto_email_regexp_validates_like_rails() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"people\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n",
        )
        .write(
            "app/models/person.rb",
            "class Person < ApplicationRecord\n  validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }\nend\n",
        )
        .run_ruby(
            r#"values = ["a@b.co", "user.name+tag@example.org", "no-at-sign", "a@b", ""]
got = values.map do |email|
  person = Person.new(email: email)
  person.valid?
  person.errors.to_a
end
want = [[], [], ["Email is invalid"], [], ["Email is invalid"]]
raise "errors: #{got.inspect}" unless got == want
puts "stdlib email regexp passed"
"#,
        )
        .assert_passes();
}

/// The folded pattern (`\A`/`\z` anchors, a character class holding `#`,
/// `/`, a backtick and `{|}`) reaches every backend's own regex emitter
/// exactly as the same literal written inline would; none rejects it.
#[test]
fn uri_mailto_email_regexp_emits_cleanly_on_every_backend_target() {
    use roundhouse::project::BuildTarget::*;
    for target in [Ruby, Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Crystal, Elixir, Roda] {
        let (_emitted, errors) = emit_and_run::real_blog()
            .edit(
                "db/schema.rb",
                "    t.string \"title\"\n",
                "    t.string \"title\"\n    t.string \"email\"\n",
            )
            .edit(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                "class Article < ApplicationRecord\n  validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }\n",
            )
            .emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}

/// Ruby's constant lookup also searches the class's ancestors: when
/// `ApplicationRecord` defines its own `URI`, an unrooted
/// `URI::MailTo::EMAIL_REGEXP` in a model could name it, so that
/// spelling stays the reported gap. The rooted spelling still folds.
#[test]
fn an_app_ancestor_defining_uri_keeps_the_unrooted_gap() {
    let warnings = emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\n\n  module URI\n  end\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"people\", force: :cascade do |t|\n    t.string \"email\"\n    t.string \"backup_email\"\n  end\nend\n",
        )
        .write(
            "app/models/person.rb",
            "class Person < ApplicationRecord\n  validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }\n  validates :backup_email, format: { with: ::URI::MailTo::EMAIL_REGEXP }\nend\n",
        )
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("validates :email, format: with:")), "{warnings:?}");
    assert!(!warnings.iter().any(|w| w.contains("validates :backup_email,")), "{warnings:?}");
}

/// Without such an ancestor the unrooted spelling folds as before.
#[test]
fn a_plain_application_record_still_folds_the_unrooted_spelling() {
    let warnings = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"people\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n",
        )
        .write(
            "app/models/person.rb",
            "class Person < ApplicationRecord\n  validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }\nend\n",
        )
        .ingest_warnings();
    assert!(!warnings.iter().any(|w| w.contains("validates :email,")), "{warnings:?}");
}

/// The same holds for an included module that defines `URI`.
#[test]
fn an_included_concern_defining_uri_keeps_the_unrooted_gap() {
    let warnings = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "app/models/concerns/contactable.rb",
            "module Contactable\n  extend ActiveSupport::Concern\n\n  URI = Object.new\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"people\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n",
        )
        .write(
            "app/models/person.rb",
            "class Person < ApplicationRecord\n  include Contactable\n\n  validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }\nend\n",
        )
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("validates :email, format: with:")), "{warnings:?}");
}

/// An app that defines or reopens the top-level `URI` itself may have
/// replaced the standard-library value, so neither spelling folds.
#[test]
fn an_app_defining_top_level_uri_keeps_both_spellings_a_gap() {
    for (file, source) in [
        ("lib/uri_mail_to.rb", "module URI\n  module MailTo\n    EMAIL_REGEXP = /x/\n  end\nend\n"),
        ("lib/uri_stub.rb", "URI = Object.new\n"),
    ] {
        let warnings = emit_and_run::empty_app()
            .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
            .write(file, source)
            .write(
                "db/schema.rb",
                "ActiveRecord::Schema.define do\n  create_table \"people\", force: :cascade do |t|\n    t.string \"email\"\n    t.string \"backup_email\"\n  end\nend\n",
            )
            .write(
                "app/models/person.rb",
                "class Person < ApplicationRecord\n  validates :email, format: { with: URI::MailTo::EMAIL_REGEXP }\n  validates :backup_email, format: { with: ::URI::MailTo::EMAIL_REGEXP }\nend\n",
            )
            .ingest_warnings();
        assert!(warnings.iter().any(|w| w.contains("validates :email, format: with:")), "{file}: {warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("validates :backup_email, format: with:")), "{file}: {warnings:?}");
    }
}
