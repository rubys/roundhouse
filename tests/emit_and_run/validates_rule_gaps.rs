use super::emit_and_run;

/// A `validates` call with a rule this build does not lower keeps its
/// other rules, as Rails runs each validator on its own: a blank value
/// `presence:` rejects is rejected on both sides. The rule with the gap is
/// dropped whole, never just its option: without `conditions:` the
/// uniqueness check below would reject the archived duplicates Rails
/// accepts. A message option the build does not lower (`too_long:`) keeps
/// its rule, with the default message. A call-level option it does not
/// lower (`on: :publish`) reaches every rule, so the call is dropped, and
/// a blank subtitle saves, as in Rails. An acceptance rule kept beside a
/// dropped key keeps the accessor ingest gives its undeclared attribute.
/// Answers are Rails 8.1.4's for the same models and records.
const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "posts", force: :cascade do |t|
    t.string "title"
    t.boolean "archived", default: false
  end

  create_table "tags", force: :cascade do |t|
    t.string "name"
    t.string "slug"
    t.boolean "archived", default: false
  end

  create_table "comments", force: :cascade do |t|
    t.string "body"
    t.string "author"
    t.string "signature"
    t.integer "rank"
    t.string "email"
    t.string "subtitle"
  end
end
"#;

const POST: &str = r#"class Post < ApplicationRecord
  validates :title, presence: true, uniqueness: { conditions: -> { where(archived: false) } }
end
"#;

const TAG: &str = r#"class Tag < ApplicationRecord
  validates :name, :slug, presence: true, uniqueness: { conditions: -> { where(archived: false) }, case_sensitive: false }
end
"#;

const COMMENT: &str = r#"class Comment < ApplicationRecord
  BODY_RANGE = 1..20

  validates :body, presence: true, length: { in: BODY_RANGE }
  validates :author, presence: true, length: { in: 1..20, too_long: "is long", too_short: "is short" }
  validates :signature, presence: true, length: { maximum: ->(_comment) { 20 } }
  validates :rank, presence: true, numericality: { other_than: 0 }
  validates :email, presence: true, email: true
  validates :subtitle, presence: true, length: { maximum: 5 }, on: :publish
  validates :terms_of_service, acceptance: true, nowhere: true
end
"#;

/// Gems' validators in Rails; the build has no `EmailValidator` and no
/// `NowhereValidator`.
const GEM_VALIDATORS: &str = r#"class EmailValidator < ActiveModel::EachValidator
  def validate_each(record, attribute, value)
    record.errors.add(attribute, :invalid) unless value.to_s.include?("@")
  end
end
class NowhereValidator < ActiveModel::EachValidator
  def validate_each(record, attribute, value)
  end
end
"#;

const SCRIPT: &str = r##"def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def blank_error?(record, attr)
  record.valid?
  record.errors.to_a.include?("#{attr.to_s.capitalize} can't be blank")
end
check("blank title", blank_error?(Post.new(title: ""), :title), true)
check("nil title", blank_error?(Post.new(title: nil), :title), true)
Post.create!(title: "Hello", archived: true)
check("a duplicate outside the conditions", Post.new(title: "Hello").valid?, true)
Post.create!(title: "Hello")
check("blank name beside a second attribute", blank_error?(Tag.new(name: "", slug: "s"), :name), true)
check("blank slug", blank_error?(Tag.new(name: "n", slug: ""), :slug), true)
Tag.create!(name: "Ruby", slug: "ruby", archived: true)
check("a case-insensitive duplicate outside the conditions", Tag.new(name: "RUBY", slug: "RUBY").valid?, true)
base = { body: "b", author: "a", signature: "s", rank: 1, email: "a@b", subtitle: "" }
check("a comment with a blank subtitle", Comment.new(base).valid?, true)
Comment.create!(base)
check("a long subtitle", Comment.new(base.merge(subtitle: "a long subtitle")).valid?, true)
[[:body, ""], [:author, ""], [:signature, ""], [:rank, nil], [:email, ""]].each do |attr, value|
  check("blank #{attr}", blank_error?(Comment.new(base.merge(attr => value)), attr), true)
end
check("an author too long", Comment.new(base.merge(author: "x" * 21)).valid?, false)
check("an author of twenty", Comment.new(base.merge(author: "x" * 20)).valid?, true)
declined = Comment.new(base)
declined.terms_of_service = "0"
declined.valid?
check("declined terms", declined.errors.to_a, ["Terms of service must be accepted"])
accepted = Comment.new(base)
accepted.terms_of_service = "1"
check("accepted terms", accepted.valid?, true)
puts "rule gaps passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/post.rb", POST)
        .write("app/models/tag.rb", TAG)
        .write("app/models/comment.rb", COMMENT)
}

#[test]
fn a_call_keeps_its_rules_beside_a_gap_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("rule gaps passed"), "{}", run.stdout);
}

/// The same script against activerecord 8.1.4 on sqlite3; skipped when
/// the gems aren't installed.
#[test]
fn rails_answers_the_rule_gaps_script() {
    let probe = r#"gem "activerecord", "8.1.4"; require "active_record"; require "sqlite3""#;
    if !std::process::Command::new("ruby").args(["-e", probe]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with activerecord 8.1.4 and sqlite3 not available");
        return;
    }
    let script = format!(
        r##"gem "activerecord", "8.1.4"
require "active_record"
require "sqlite3"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
{SCHEMA}
{GEM_VALIDATORS}
{APPLICATION_RECORD}
{POST}
{TAG}
{COMMENT}
{SCRIPT}"##
    );
    let output = std::process::Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("rule gaps passed"),
        "activerecord 8.1.4 disagrees\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}

/// Each warning says what it drops: the one rule, naming the rules the
/// call keeps, or the whole call, naming the rules lost with it. A kept
/// rule's message option says only the error text differs.
#[test]
fn the_warnings_say_what_is_dropped() {
    let warnings = app().ingest_warnings();
    let has = |construct: &str, text: &str| {
        assert!(
            warnings.iter().any(|w| w.contains(construct) && w.contains(text)),
            "{construct} / {text}: {warnings:#?}"
        )
    };
    let kept = "only the `{}:` rule is dropped, the call keeps `presence:`";
    for (construct, rule) in [
        ("validates :title, uniqueness: conditions:", "uniqueness"),
        ("validates :name, uniqueness: conditions:", "uniqueness"),
        ("validates :slug, uniqueness: conditions:", "uniqueness"),
        ("validates :body, length: in:", "length"),
        ("validates :signature, length: maximum:", "length"),
        ("validates :rank, numericality: other_than:", "numericality"),
        ("validates :email, email:", "email"),
    ] {
        has(construct, &kept.replace("{}", rule));
    }
    for construct in ["validates :author, length: too_long:", "validates :author, length: too_short:"] {
        has(construct, "the rule runs with its default message");
    }
    has("validates :subtitle, on:", "the whole call is dropped, including `presence:`, `length:`");
    let terms: Vec<&String> = warnings.iter().filter(|w| w.contains("validates :terms_of_service, nowhere:")).collect();
    assert!(
        terms.len() == 1 && terms[0].contains("only the `nowhere:` rule is dropped, the call keeps `acceptance:`"),
        "one warning for the call, not one for its accessor too: {terms:#?}"
    );
}
