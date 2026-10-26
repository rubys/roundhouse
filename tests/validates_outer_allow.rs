//! `validates :x, <validators>, allow_nil: true` / `allow_blank: true`: Rails
//! merges the call's options under each validator's own, so the option
//! applies to every validator on the line, and a validator's own option
//! wins (activerecord 8.1.4):
//! - `allow_nil:` skips nil only; `""` is still checked;
//! - `allow_blank:` skips nil, `""` and `"  "`;
//! - a validator's own `allow_nil: false` keeps nil checked;
//! - `if:` still decides whether the line runs at all;
//! - an inclusion's or exclusion's own `allow_nil:` / `allow_blank:`
//!   applies to that validator alone.
//!
//! A line whose validators store no flag of their own (inclusion,
//! exclusion) was refused ("allow_nil: not supported … condition not
//! lowered"); each such rule now runs only when the value is not nil (or
//! not blank).

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const COLUMNS: &str = "create_table \"articles\", force: :cascade do |t|\n    t.string \"tone\"\n    t.string \"slug\"\n    t.string \"code\"\n    t.string \"word\"\n    t.string \"gate\"\n    t.string \"note\"\n    t.string \"pick\"\n    t.string \"handle\"\n    t.string \"short\"";

const MODEL: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  validates :tone, inclusion: { in: %w(x y) }, length: { maximum: 1 }, allow_nil: true
  validates :slug, inclusion: { in: %w(x y) }, format: { with: /\A[a-z]+\z/ }, allow_blank: true
  validates :code, inclusion: { in: %w(xx yy) }, length: { minimum: 2, allow_nil: false }, allow_nil: true
  validates :word, exclusion: { in: %w(admin) }, allow_blank: true
  validates :gate, inclusion: { in: %w(x) }, allow_nil: true, if: -> { note == "on" }
  validates :pick, inclusion: { in: %w(x), allow_nil: true }
  validates :handle, exclusion: { in: %w(admin), allow_blank: true }, inclusion: { in: %w(ok admin) }
  validates :short, length: { maximum: 1 }, allow_blank: true
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", COLUMNS)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            MODEL,
        )
}

const TEST: &str = r#"require "test_helper"

class ArticleOuterAllowTest < ActiveSupport::TestCase
  VALID = { title: "Outer", body: "A body long enough to validate.", tone: "x", slug: "x", code: "xx", word: "hello", gate: "x", pick: "x", handle: "ok" }.freeze

  def errors_for(**attrs)
    article = Article.new(**VALID, **attrs)
    article.valid? ? [] : article.errors.full_messages
  end

  test "the baseline is valid" do
    assert_equal [], errors_for
  end

  test "allow_nil skips nil for every validator on the line" do
    assert_equal [], errors_for(tone: nil)
  end

  test "allow_nil still checks an empty string" do
    assert_equal ["Tone is not included in the list"], errors_for(tone: "")
  end

  test "both validators still run on a value" do
    assert_equal ["Tone is not included in the list", "Tone is too long (maximum is 1 character)"], errors_for(tone: "xy")
  end

  test "allow_blank skips nil, empty and whitespace" do
    assert_equal [], errors_for(slug: nil)
    assert_equal [], errors_for(slug: "")
    assert_equal [], errors_for(slug: "  ")
    assert_equal [], errors_for(word: "  ")
    assert_equal [], errors_for(short: "  ")
  end

  test "allow_blank still checks a value" do
    assert_equal ["Slug is not included in the list", "Slug is invalid"], errors_for(slug: "X1")
    assert_equal ["Word is reserved"], errors_for(word: "admin")
    assert_equal ["Short is too long (maximum is 1 character)"], errors_for(short: "ab")
  end

  test "a validator's own allow_nil: false wins" do
    assert_equal ["Code is too short (minimum is 2 characters)"], errors_for(code: nil)
  end

  test "a validator's own allow_nil and allow_blank apply to it alone" do
    assert_equal [], errors_for(pick: nil)
    assert_equal ["Pick is not included in the list"], errors_for(pick: "")
    assert_equal ["Handle is not included in the list"], errors_for(handle: "")
    assert_equal ["Handle is reserved"], errors_for(handle: "admin")
  end

  test "if: still decides whether the line runs" do
    assert_equal [], errors_for(gate: nil, note: "on")
    assert_equal ["Gate is not included in the list"], errors_for(gate: "z", note: "on")
    assert_equal [], errors_for(gate: "z", note: "off")
  end
end
"#;

#[test]
fn an_outer_allow_option_applies_to_every_validator_on_ruby() {
    app().write("test/models/article_outer_allow_test.rb", TEST).run_test("test/models/article_outer_allow_test.rb").assert_passes();
}

const SPINEL_SCRIPT: &str = r##"
valid = { title: "Outer", body: "A body long enough to validate.", tone: "x", slug: "x", code: "xx", word: "hello", gate: "x", pick: "x", handle: "ok" }
def errors_for(valid, **attrs)
  article = Article.new(**valid, **attrs)
  article.valid? ? [] : article.errors.to_a
end
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
check("baseline", errors_for(valid), [])
check("allow_nil nil", errors_for(valid, tone: nil), [])
check("allow_nil empty", errors_for(valid, tone: ""), ["Tone is not included in the list"])
check("allow_blank spaces", errors_for(valid, slug: "  "), [])
check("allow_blank maximum-only length", errors_for(valid, short: "  "), [])
check("allow_blank value", errors_for(valid, slug: "X1"), ["Slug is not included in the list", "Slug is invalid"])
check("own allow_nil false", errors_for(valid, code: nil), ["Code is too short (minimum is 2 characters)"])
check("if off", errors_for(valid, gate: "z", note: "off"), [])
check("own allow_blank", errors_for(valid, handle: ""), ["Handle is not included in the list"])
puts "outer allow passed"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_outer_allow_option_applies_to_every_validator_on_spinel() {
    let run = app().run_spinel_with_db(SPINEL_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("outer allow passed"), "{}", run.stdout);
}

/// The strict targets lower the same guards (`!x.nil?`, `!x.blank?`).
#[test]
fn a_strict_target_emits_the_guarded_rules() {
    for target in [roundhouse::project::BuildTarget::Rust, roundhouse::project::BuildTarget::Typescript] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
