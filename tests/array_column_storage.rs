//! A PostgreSQL array column on the ruby family (#1272). Rails reads it as
//! an `Array` of the column's type. Measured on activerecord 8.1.4 with
//! PostgreSQL:
//! - a nullable column with no default reads nil, new or loaded;
//! - `default: []` reads `[]` on a new record and after a load;
//! - an assigned `[1, 2]` reloads as `[1, 2]`;
//! - `attributes` and `[]` give the same Array;
//! - an in-place `labels << "z"` is saved by the next `save!`;
//! - `where(id: tag_ids)` is an IN list over the Array's elements;
//! - `pluck(:tag_ids)` gives each row's Array.
//!
//! The ruby and spinel targets store the column in SQLite as JSON text and
//! keep the decoded Array in the record, so each read is that Array.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const COLUMNS: &str = "create_table \"articles\", force: :cascade do |t|\n    t.bigint \"tag_ids\", array: true\n    t.string \"labels\", array: true, default: [], null: false\n    t.string \"aliases\", array: true\n    t.bigint \"owner_ids\", array: true, null: false";

const MODEL_METHODS: &str = "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n\n  def first_label\n    labels.first\n  end\n\n  def tagged?(id)\n    tag_ids.present? && tag_ids.include?(id)\n  end\n\n  def peer_titles\n    Article.where(id: tag_ids).map(&:title).sort\n  end";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", COLUMNS)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            MODEL_METHODS,
        )
}

const TEST: &str = r#"require "test_helper"

class ArticleArrayColumnTest < ActiveSupport::TestCase
  test "a new record reads nil or its empty default" do
    article = Article.new
    assert_nil article.tag_ids
    assert_equal [], article.labels
    assert_nil article.first_label
    assert_nil article.aliases
  end

  test "an array round-trips and every read gives it" do
    article = Article.create!(title: "Arrays", body: "A body long enough to validate.", tag_ids: [1, 2], labels: ["x", "y"], aliases: ["a"], owner_ids: [7])
    found = Article.find(article.id)
    assert_equal [1, 2], found.tag_ids
    assert_equal ["x", "y"], found.labels
    assert_equal ["a"], found.aliases
    assert_equal [7], found.owner_ids
    assert_equal [1, 2], found[:tag_ids]
    assert_equal ["x", "y"], found.attributes["labels"]
    assert_equal "x", found.first_label
    assert found.tagged?(2)
    assert_not found.tagged?(3)
  end

  test "a where over the Array is an IN list, and pluck gives the Array" do
    one = Article.create!(title: "One", body: "A body long enough to validate.")
    two = Article.create!(title: "Two", body: "A body long enough to validate.")
    Article.create!(title: "Three", body: "A body long enough to validate.")
    tagged = Article.create!(title: "Tagged", body: "A body long enough to validate.", tag_ids: [one.id, two.id])
    assert_equal ["One", "Two"], Article.find(tagged.id).peer_titles
    assert_equal [[one.id, two.id]], Article.where(id: tagged.id).pluck(:tag_ids)
    assert_equal [[[one.id, two.id], []]], Article.where(id: tagged.id).pluck(:tag_ids, :labels)
  end

  test "an unset column loads as nil or its default" do
    article = Article.create!(title: "Bare", body: "A body long enough to validate.")
    found = Article.find(article.id)
    assert_nil found.tag_ids
    assert_equal [], found.labels
  end

  test "an update and an in-place append are saved" do
    article = Article.create!(title: "Grow", body: "A body long enough to validate.", labels: ["a"])
    article.update!(labels: ["c"])
    assert_equal ["c"], Article.find(article.id).labels
    found = Article.find(article.id)
    found.labels << "z"
    found.save!
    assert_equal ["c", "z"], Article.find(article.id).labels
  end
end
"#;

#[test]
fn an_array_column_round_trips_on_ruby() {
    app().write("test/models/article_array_column_test.rb", TEST).run_test("test/models/article_array_column_test.rb").assert_passes();
}
