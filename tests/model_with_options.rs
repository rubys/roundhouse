//! `with_options` in a model class body. activerecord 8.1.4 (oracle
//! `model_with_options.rb`): the options merge into each call in the block
//! with the call's own keys winning, so an inner `if:` REPLACES an outer
//! `if:` and an inner `on:` an outer `on:`, an inner `unless:` sits beside
//! an outer `if:`, an inner `class_name:` overrides the outer one, nested
//! blocks merge outward in, and `do |o|` merges into the calls on `o`.
//!
//! The block was one unlowered statement, and every call in it was dropped.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const COLUMNS: &str = "create_table \"articles\", force: :cascade do |t|\n    t.string \"a\"\n    t.string \"b\"\n    t.string \"c\"\n    t.string \"d\"\n    t.boolean \"local\"\n    t.boolean \"flag\"\n    t.integer \"boss_id\"\n    t.integer \"mentor_id\"";

const MODEL: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  with_options if: :local? do
    validates :a, presence: true
    validates :b, presence: true, if: :flag?
    validates :c, presence: true, unless: :flag?
  end

  with_options on: :create do
    validates :d, presence: true, on: :update
  end

  with_options class_name: "Article", optional: true do
    belongs_to :boss
    belongs_to :mentor, class_name: "Comment", foreign_key: :mentor_id
  end

  with_options(on: :create) do |o|
    o.validates :a, length: { maximum: 3 }
  end

  with_options if: :local? do
    with_options unless: :flag? do
      validates :b, length: { minimum: 2 }
    end
  end
"#;

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def article(**attrs)
  Article.new(title: "With options", body: "A body long enough to validate.", **attrs)
end
check("outer if false skips the block", article(local: false).valid?, true)
check("outer if true runs it", article(local: true, b: "bb", c: "x").valid?, false)
check("all present", article(local: true, a: "ab", b: "bb", c: "x").valid?, true)
check("an inner if replaces the outer if", article(local: false, flag: true).valid?, false)
check("an inner unless sits beside the outer if", article(local: true, flag: true, a: "ab", b: "bb").valid?, true)
check("the |o| form merges on: :create", article(local: false, a: "abcd").valid?, false)
saved = article(local: false, a: "ab")
saved.save!
saved.a = "abcd"
check("on: :create skips an update", saved.valid?, false)
saved.d = "dd"
check("an inner on: :update replaces the outer on: :create", saved.valid?, true)
boss = article(local: false)
boss.save!
check("class_name merged", Article.find(article(local: false, boss_id: boss.id).tap(&:save!).id).boss.id, boss.id)
comment = Comment.create!(article_id: boss.id, commenter: "c", body: "a comment body")
check("an inner class_name overrides", article(local: false, mentor_id: comment.id).mentor.id, comment.id)
puts "model with_options passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", COLUMNS)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n", MODEL)
}

#[test]
fn a_models_with_options_merges_into_its_calls_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("model with_options passed"), "{}", run.stdout);
}

/// Options that are not a literal hash (a constant here, a local alike)
/// are not read, so the whole block stays a gap that names why.
#[test]
fn non_literal_options_stay_a_gap_that_names_why() {
    let warnings = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  OPTS = { allow_nil: true }\n  with_options OPTS do\n    validates :title, presence: true\n  end\n",
        )
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("with_options") && w.contains("a local or a constant is not read")), "{warnings:#?}");
}

/// A call in the block with a block of its own: Rails merges into its
/// nested calls too, which the replay does not model, so the whole block
/// stays a gap.
#[test]
fn a_call_with_its_own_block_keeps_the_block_a_gap() {
    let warnings = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  with_options if: :title? do\n    validates :body, presence: true\n    validate do\n      errors.add(:body, \"x\")\n    end\n  end\n",
        )
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("with_options") && w.contains("`validate` with a block is not composed")), "{warnings:#?}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_models_with_options_merges_into_its_calls_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("model with_options passed"), "{}", run.stdout);
}
