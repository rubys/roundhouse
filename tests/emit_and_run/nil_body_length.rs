//! A nil value fails `length: { minimum: }` the way Rails 8.1's
//! LengthValidator does. The check used to sit inside `unless @attr.nil?`,
//! so a nil body skipped the minimum and only the presence rule fired.
use super::emit_and_run;

const NIL_BODY: &str = r#"
article = Article.new(title: "Valid Title", body: nil)
raise "nil body was valid #{article.errors.inspect}" if article.valid?
raise "nil body #{article.errors.inspect}" unless article.errors == ["Body can't be blank", "Body is too short (minimum is 10 characters)"]

blank = Article.new(title: "Valid Title", body: "")
blank.valid?
raise "blank #{blank.errors.inspect}" unless blank.errors == ["Body can't be blank", "Body is too short (minimum is 10 characters)"]

short = Article.new(title: "Valid Title", body: "Short")
short.valid?
raise "short #{short.errors.inspect}" unless short.errors == ["Body is too short (minimum is 10 characters)"]

ok = Article.new(title: "Valid Title", body: "long enough")
raise "ok #{ok.errors.inspect}" unless ok.valid?
puts "nil body length passed"
"#;

const ALLOW_NIL: &str = r#"
skipped = Article.new(title: "Valid Title", body: nil)
raise "allow_nil rejected nil #{skipped.errors.inspect}" unless skipped.valid?

short = Article.new(title: "Valid Title", body: "Short")
raise "allow_nil accepted a short body #{short.errors.inspect}" if short.valid?
raise "short #{short.errors.inspect}" unless short.errors == ["Body is too short (minimum is 10 characters)"]
puts "allow_nil length passed"
"#;

#[test]
fn a_nil_body_fails_minimum_length() {
    emit_and_run::real_blog().run_ruby(NIL_BODY).assert_passes();
}

#[test]
fn allow_nil_skips_the_minimum_and_still_rejects_a_short_string() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\n",
            "  validates :body, length: { minimum: 10 }, allow_nil: true\n",
        )
        .run_ruby(ALLOW_NIL)
        .assert_passes();
}
