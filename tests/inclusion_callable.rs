//! `inclusion:` / `exclusion:` with a callable `in:`. Rails' `Clusivity`
//! calls a lambda or proc with the record and calls a Symbol on it
//! (activerecord 8.1.4):
//! - `in: ->(a) { a.class::CATEGORIES }` reads the record's own class, so
//!   a subclass's constant applies to its records;
//! - `in: -> { TONES }` runs its body with the record as `self`;
//! - `in: :allowed_moods` calls that method of the record;
//! - a set that is nil raises `NoMethodError` (`include?` on nil);
//! - the messages are "is not included in the list" and "is reserved".
//!
//! Ingest refused all of them ("inclusion: in"); the rule now carries the
//! set and the record answers it when it validates.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const COLUMNS: &str = "create_table \"articles\", force: :cascade do |t|\n    t.string \"type\"\n    t.string \"category\"\n    t.string \"tone\"\n    t.string \"mood\"\n    t.string \"word\"\n    t.string \"code\"";

const MODEL: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  CATEGORIES = %w(news opinion).freeze
  TONES = %w(calm loud).freeze

  validates :category, inclusion: { in: ->(a) { a.class::CATEGORIES } }
  validates :tone, inclusion: { in: -> { TONES } }
  validates :mood, inclusion: { in: :allowed_moods }
  validates :word, exclusion: { in: ->(record) { record.reserved_words } }
  validates :code, inclusion: { in: ->(a) { a.no_codes } }, if: -> { code == "check" }

  def allowed_moods
    %w(happy)
  end

  def reserved_words
    %w(admin root)
  end

  def no_codes
    nil
  end
"#;

const FEATURE: &str = "class Feature < Article\n  CATEGORIES = %w(spotlight).freeze\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", COLUMNS)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            MODEL,
        )
        .write("app/models/feature.rb", FEATURE)
}

const TEST: &str = r#"require "test_helper"

class ArticleInclusionCallableTest < ActiveSupport::TestCase
  VALID = { title: "Callable", body: "A body long enough to validate.", category: "news", tone: "calm", mood: "happy", word: "hello" }.freeze

  def article(**attrs)
    Article.new(**VALID, **attrs)
  end

  test "a lambda given the record reads the record's class" do
    assert article(category: "news").valid?
    record = article(category: "spotlight")
    assert_not record.valid?
    assert_equal ["Category is not included in the list"], record.errors.full_messages
  end

  test "a subclass's constant applies to its records" do
    assert Feature.new(**VALID, category: "spotlight").valid?
    assert_not Feature.new(**VALID, category: "news").valid?
  end

  test "a lambda without a parameter runs on the record" do
    assert article(tone: "calm").valid?
    assert_not article(tone: "quiet").valid?
  end

  test "a symbol calls the record's method" do
    assert article(mood: "happy").valid?
    assert_not article(mood: "sad").valid?
  end

  test "exclusion asks the callable too" do
    assert article(word: "hello").valid?
    record = article(word: "admin")
    assert_not record.valid?
    assert_equal ["Word is reserved"], record.errors.full_messages
  end

  test "a nil set raises as Rails does" do
    assert_raises(NoMethodError) { article(code: "check").valid? }
  end
end
"#;

#[test]
fn a_callable_in_is_asked_on_the_record_on_ruby() {
    app().write("test/models/article_inclusion_callable_test.rb", TEST).run_test("test/models/article_inclusion_callable_test.rb").assert_passes();
}

const SPINEL_SCRIPT: &str = r##"
valid = { title: "Callable", body: "A body long enough to validate.", category: "news", tone: "calm", mood: "happy", word: "hello" }
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
check("record's class", Article.new(**valid).valid?, true)
check("not in the record's class", Article.new(**valid, category: "spotlight").valid?, false)
check("subclass", Feature.new(**valid, category: "spotlight").valid?, true)
check("subclass rejects", Feature.new(**valid, category: "news").valid?, false)
check("arity 0", Article.new(**valid, tone: "quiet").valid?, false)
check("symbol", Article.new(**valid, mood: "sad").valid?, false)
reserved = Article.new(**valid, word: "admin")
check("exclusion", reserved.valid?, false)
check("exclusion message", reserved.errors.to_a, ["Word is reserved"])
raised = begin
  Article.new(**valid, code: "check").valid?
  "no error"
rescue NoMethodError
  "NoMethodError"
end
check("nil set", raised, "NoMethodError")
puts "callable in: passed"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_callable_in_is_asked_on_the_record_on_spinel() {
    let run = app().run_spinel_with_db(SPINEL_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("callable in: passed"), "{}", run.stdout);
}

/// A strict target has no class object to ask, so a callable that reads
/// `record.class::X` is refused where it is written, though the
/// validation is lowered after the analysis that refuses it elsewhere.
#[test]
fn a_strict_target_refuses_a_class_value_read_in_the_set() {
    let (_emitted, errors) = app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("Module protocol") && e.contains("const_get")), "{errors:#?}");
}

// Rails 8.1.4 (oracle `inclusion_callable_cr.rb`): a set check asks the
// set for each element of an Array value (`value.all?`), so an exclusion
// rejects it only when every element is reserved and `[]` is in every set;
// a nil value is asked of the set as it is. A nested block that names the
// lambda's parameter rebinds it.
const ARRAY_COLUMNS: &str = "create_table \"articles\", force: :cascade do |t|\n    t.string \"shade\"\n    t.string \"tags\", array: true\n    t.string \"marks\", array: true";

const ARRAY_MODEL: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  validates :shade, inclusion: { in: ->(record) { %w(red).map { |record| record } } }, allow_nil: true
  validates :tags, inclusion: { in: %w(a b) }
  validates :marks, exclusion: { in: ->(r) { %w(x) } }
"#;

fn array_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", ARRAY_COLUMNS)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            ARRAY_MODEL,
        )
}

const ARRAY_TEST: &str = r#"require "test_helper"

class ArticleArrayClusivityTest < ActiveSupport::TestCase
  def article(**attrs)
    Article.new(title: "Arrays", body: "A body long enough to validate.", tags: %w(a), **attrs)
  end

  test "a nested block's own parameter is not the record" do
    assert article(shade: "red").valid?
    assert_not article(shade: "blue").valid?
  end

  test "an Array value is included when every element is" do
    assert article(tags: %w(a b)).valid?
    assert article(tags: []).valid?
    record = article(tags: %w(a c))
    assert_not record.valid?
    assert_equal ["Tags is not included in the list"], record.errors.full_messages
    assert_not article(tags: nil).valid?
  end

  test "an Array value is reserved only when every element is" do
    assert article(marks: %w(x y)).valid?
    assert article(marks: nil).valid?
    assert_not article(marks: %w(x)).valid?
    assert_not article(marks: []).valid?
  end
end
"#;

#[test]
fn an_array_value_is_checked_element_by_element_on_ruby() {
    array_app().write("test/models/article_array_clusivity_test.rb", ARRAY_TEST).run_test("test/models/article_array_clusivity_test.rb").assert_passes();
}

/// A one-parameter callable's body keeps the class body's `self` in Rails
/// (`value.call(record)`), so a receiverless call in it runs on the class.
/// Lowered into the record's validation it would run on the record, so it
/// stays a gap.
#[test]
fn a_one_parameter_callable_calling_its_class_stays_a_gap() {
    let warnings = emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", "create_table \"articles\", force: :cascade do |t|\n    t.string \"mood\"")
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  def self.allowed_moods = %w(happy)\n  validates :mood, inclusion: { in: ->(r) { allowed_moods } }\n",
        )
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("inclusion: in")), "{warnings:#?}");
}

const ARRAY_SPINEL_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def article(shade: nil, tags: ["a"], marks: nil)
  Article.new(title: "Arrays", body: "A body long enough to validate.", shade: shade, tags: tags, marks: marks)
end
check("nested block parameter", article(shade: "red").valid?, true)
check("nested block parameter rejects", article(shade: "blue").valid?, false)
check("all included", article(tags: ["a", "b"]).valid?, true)
check("empty included", article(tags: []).valid?, true)
check("one not included", article(tags: ["a", "c"]).valid?, false)
check("nil not included", article(tags: nil).valid?, false)
check("not all reserved", article(marks: ["x", "y"]).valid?, true)
check("all reserved", article(marks: ["x"]).valid?, false)
check("empty reserved", article(marks: []).valid?, false)
puts "array clusivity passed"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_array_value_is_checked_element_by_element_on_spinel() {
    let run = array_app().run_spinel_with_db(ARRAY_SPINEL_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("array clusivity passed"), "{}", run.stdout);
}

// Rails 8.1.4 (oracle `inclusion_numbered.rb`): `-> { _1.allowed }` has
// arity 1 and `-> { it.tones }` names the same parameter, so both are
// called with the record; `-> { [_1, _2] }` takes two and Rails raises
// ArgumentError.
const NUMBERED_MODEL: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  validates :mood, inclusion: { in: -> { _1.allowed } }, allow_nil: true
  validates :tone, inclusion: { in: -> { it.tones } }, allow_nil: true

  def allowed
    %w(happy)
  end

  def tones
    %w(calm)
  end
"#;

const NUMBERED_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
base = { title: "Numbered", body: "A body long enough to validate." }
check("_1 is the record", Article.new(**base, mood: "happy").valid?, true)
check("_1 rejects", Article.new(**base, mood: "sad").valid?, false)
check("it is the record", Article.new(**base, tone: "calm").valid?, true)
check("it rejects", Article.new(**base, tone: "loud").valid?, false)
puts "numbered parameters passed"
"##;

#[test]
fn a_numbered_or_it_parameter_is_the_record_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", "create_table \"articles\", force: :cascade do |t|\n    t.string \"mood\"\n    t.string \"tone\"")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n", NUMBERED_MODEL)
        .run_ruby(NUMBERED_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("numbered parameters passed"), "{}", run.stdout);
}

/// Two numbered parameters: Rails raises ArgumentError calling it with
/// the record, so it stays a gap.
#[test]
fn two_numbered_parameters_stay_a_gap() {
    let warnings = emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", "create_table \"articles\", force: :cascade do |t|\n    t.string \"word\"")
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  validates :word, inclusion: { in: -> { [_1, _2] } }\n",
        )
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("inclusion: in")), "{warnings:#?}");
}
