//! A `has_many :through` reads the rows of its source association: the
//! association on the through model named by `source:`, or by the
//! association's own name when `source:` is omitted.
//!
//! The class comes from that association. `has_many :article_remarks,
//! through: :articles, source: :remarks` on a shelf, where `Article` has
//! `has_many :remarks, class_name: "Comment"`, yields `Comment` rows, not
//! rows of a `Remark` class that does not exist.
//!
//! The join comes from that association too, found by name and not by
//! class. A `Link` with two `belongs_to` to `Article` gives other rows
//! for `source: :target_article` than for `source: :source_article`.

use super::emit_and_run;

const SCHEMA_FIND: &str = "  create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n";
const SCHEMA_REPLACE: &str = "  create_table \"shelves\", force: :cascade do |t|\n    t.string \"name\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"links\", force: :cascade do |t|\n    t.integer \"source_article_id\"\n    t.integer \"target_article_id\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"articles\", force: :cascade do |t|\n    t.integer \"shelf_id\"\n    t.string \"title\"\n";

const TEST: &str = r#"require "test_helper"

class ThroughSourceClassTest < ActiveSupport::TestCase
  test "a through association reads the class of its source association" do
    north = Shelf.create!(name: "north")
    south = Shelf.create!(name: "south")
    alpha = Article.create!(title: "Alpha", body: "Body long enough.", shelf: north)
    beta = Article.create!(title: "Beta", body: "Body long enough.", shelf: south)
    alpha.remarks.create!(commenter: "bo", body: "first")
    alpha.remarks.create!(commenter: "al", body: "second")
    beta.remarks.create!(commenter: "cy", body: "third")

    assert_equal ["al", "bo"], north.article_remarks.map(&:commenter).sort
    assert_equal ["al", "bo"], north.article_remarks.order(:commenter).pluck(:commenter)
    assert_equal ["AL", "BO"], north.article_remarks.order(:commenter).map(&:signature)
    assert_equal "Comment", north.article_remarks.first.class.name
    assert_equal ["al", "bo"], north.remarks.map(&:commenter).sort
  end

  test "a through association joins on the source association it names" do
    alpha = Article.create!(title: "Alpha", body: "Body long enough.")
    beta = Article.create!(title: "Beta", body: "Body long enough.")
    gamma = Article.create!(title: "Gamma", body: "Body long enough.")
    Link.create!(source_article: alpha, target_article: beta)
    Link.create!(source_article: gamma, target_article: alpha)

    assert_equal ["Beta"], alpha.linked_articles.map(&:title)
    assert_equal ["Gamma"], alpha.linking_articles.map(&:title)
    assert_equal [["Alpha", ["Beta"]], ["Beta", []], ["Gamma", ["Alpha"]]],
      Article.where(title: ["Alpha", "Beta", "Gamma"]).includes(:linked_articles).order(:title)
        .map { |a| [a.title, a.linked_articles.map(&:title)] }
  end
end
"#;

fn shelved_blog() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_FIND, SCHEMA_REPLACE)
        .write(
            "app/models/shelf.rb",
            "class Shelf < ApplicationRecord\n  has_many :articles\n  has_many :article_remarks, through: :articles, source: :remarks\n  has_many :remarks, through: :articles\nend\n",
        )
        .write(
            "app/models/link.rb",
            "class Link < ApplicationRecord\n  belongs_to :source_article, class_name: \"Article\"\n  belongs_to :target_article, class_name: \"Article\"\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n  belongs_to :shelf, optional: true\n  has_many :remarks, class_name: \"Comment\"\n  has_many :outgoing_links, class_name: \"Link\", foreign_key: :source_article_id\n  has_many :incoming_links, class_name: \"Link\", foreign_key: :target_article_id\n  has_many :linked_articles, through: :outgoing_links, source: :target_article\n  has_many :linking_articles, through: :incoming_links, source: :source_article\n",
        )
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article\n",
            "  belongs_to :article\n\n  def signature\n    commenter.upcase\n  end\n",
        )
}

#[test]
fn has_many_through_reads_its_source_association() {
    shelved_blog()
        .write("test/models/through_source_class_test.rb", TEST)
        .run_test("test/models/through_source_class_test.rb")
        .assert_passes();
}

/// The class each `has_many` on `Shelf` reads after ingest.
fn shelf_targets(shelf: &str, article: &str) -> std::collections::HashMap<String, String> {
    use roundhouse::dialect::Association;

    let tree = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/shelf.rb", shelf),
        ("app/models/article.rb", article),
        ("app/models/comment.rb", "class Comment < ApplicationRecord\n  belongs_to :article\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :article\nend\n"),
        ("app/models/special_comment.rb", "class SpecialComment < Comment\nend\n"),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest the shelf app");
    let shelf = app.models.iter().find(|m| m.name.0.as_str() == "Shelf").expect("Shelf model");
    shelf
        .associations()
        .filter_map(|a| match a {
            Association::HasMany { name, target, .. } => {
                Some((name.as_str().to_string(), target.0.as_str().to_string()))
            }
            _ => None,
        })
        .collect()
}

/// `class_name:` names the class of a through association, as in Rails;
/// the source association supplies the class only when nothing names it.
/// `source:` is looked up as written, and an omitted `source:` that
/// matches both the singular and the plural name is ambiguous, so the
/// guess from the name stays (Rails raises
/// `AmbiguousSourceReflectionForThroughAssociation`).
#[test]
fn through_class_follows_rails_source_reflection_rules() {
    let targets = shelf_targets(
        "class Shelf < ApplicationRecord\n  has_many :articles\n  \
         has_many :special_remarks, through: :articles, source: :remarks, class_name: \"SpecialComment\"\n  \
         has_many :article_remarks, through: :articles, source: :remarks\n  \
         has_many :singular_only, through: :articles, source: :entries\n  \
         has_many :items, through: :articles\nend\n",
        "class Article < ApplicationRecord\n  belongs_to :shelf\n  \
         has_many :remarks, class_name: \"Comment\"\n  \
         has_one :entry, class_name: \"Note\"\n  \
         has_one :item, class_name: \"Note\"\n  \
         has_many :items, class_name: \"Comment\"\nend\n",
    );
    assert_eq!(targets["special_remarks"], "SpecialComment", "{targets:?}");
    assert_eq!(targets["article_remarks"], "Comment", "{targets:?}");
    assert_eq!(targets["singular_only"], "Entry", "{targets:?}");
    assert_eq!(targets["items"], "Item", "{targets:?}");
}

/// The through reader, preload and join do not apply the scope of the
/// source association, so a scoped source refuses instead of answering
/// the rows the scope filters out.
#[test]
fn has_many_through_a_scoped_source_refuses() {
    let (_emitted, errors) = shelved_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :remarks, class_name: \"Comment\"\n",
            "  has_many :remarks, class_name: \"Comment\"\n  has_many :bo_remarks, -> { where(commenter: \"bo\") }, class_name: \"Comment\"\n",
        )
        .edit(
            "app/models/shelf.rb",
            "  has_many :remarks, through: :articles\n",
            "  has_many :remarks, through: :articles\n  has_many :bo_remarks, through: :articles\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let refused: Vec<&String> = errors.iter().filter(|e| e.contains("through_source_scope")).collect();
    assert!(
        refused.iter().any(|e| e.contains("Shelf.bo_remarks")),
        "a scoped source association must refuse: {errors:?}"
    );
    assert_eq!(refused.len(), 1, "only the scoped source refuses: {errors:?}");
}
