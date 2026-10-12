//! `has_one` / `has_many` `primary_key:` names the OWNER column the foreign
//! key matches. `has_one :source_profile, foreign_key: :source_id,
//! primary_key: :source_id` is `SourceProfile.where(source_id:
//! article.source_id)`, not `... article.id`.
//!
//! Every read of the association has to agree: the reader, `includes` (the
//! batch preload), `joins` (the ON clause), `build` / `create`, and the
//! `dependent: :destroy` cascade. The data is arranged so a path that falls
//! back to `id` returns visibly wrong rows rather than none: the articles'
//! `source_id`s (100, 200) never equal their ids, and decoy children are keyed
//! on the owners' *ids*, which an id-keyed query would pick up. An owner whose
//! key is NULL (Gamma) matches nothing, even though orphan children with a NULL
//! foreign key exist: Rails never matches a nil owner key.
//!
//! Expected values come from real Rails 8.1 (activerecord 8.1.3.1 +
//! sqlite3) running the same schema and models; the oracle script is
//! `work/_scratch/apk-oracle/oracle.rb` in the package's scratch dir.

use super::emit_and_run;

const SCHEMA_FIND: &str = "  create_table \"comments\", force: :cascade do |t|\n";
const SCHEMA_REPLACE: &str = "  create_table \"source_profiles\", force: :cascade do |t|\n    t.integer \"source_id\"\n    t.string \"name\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"source_notes\", force: :cascade do |t|\n    t.integer \"source_id\"\n    t.string \"body\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\", force: :cascade do |t|\n";

fn keyed_blog() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.text \"body\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"",
            "    t.text \"body\"\n    t.integer \"source_id\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"",
        )
        .edit("db/schema.rb", SCHEMA_FIND, SCHEMA_REPLACE)
        .write("app/models/source_profile.rb", "class SourceProfile < ApplicationRecord\nend\n")
        .write("app/models/source_note.rb", "class SourceNote < ApplicationRecord\nend\n")
}

const SETUP: &str = r#"
  def seed
    @alpha = Article.create!(title: "Alpha", body: "Body long enough.", source_id: 200)
    @beta = Article.create!(title: "Beta", body: "Body long enough.", source_id: 100)
    @gamma = Article.create!(title: "Gamma", body: "Body long enough.", source_id: nil)
    SourceProfile.create!(name: "p100", source_id: 100)
    SourceProfile.create!(name: "p200", source_id: 200)
    SourceProfile.create!(name: "decoy-for-beta", source_id: @beta.id)
    SourceProfile.create!(name: "orphan", source_id: nil)
    SourceNote.create!(body: "n100a", source_id: 100)
    SourceNote.create!(body: "n200", source_id: 200)
    SourceNote.create!(body: "n100b", source_id: 100)
    SourceNote.create!(body: "decoy-for-alpha", source_id: @alpha.id)
    SourceNote.create!(body: "orphan-note", source_id: nil)
  end

  def mine
    Article.where(title: ["Alpha", "Beta", "Gamma"])
  end
"#;

#[test]
fn has_one_and_has_many_primary_key_is_honored_on_every_read() {
    let test = format!(
        r#"require "test_helper"

class AssocPrimaryKeyTest < ActiveSupport::TestCase
{SETUP}
  test "the readers match the foreign key against the owner's primary_key" do
    seed
    assert_not_equal @alpha.id, @alpha.source_id
    assert_equal "p200", @alpha.source_profile.name
    assert_equal "p100", @beta.source_profile.name
    assert_nil @gamma.source_profile
    assert_equal ["n200"], @alpha.source_notes.map(&:body).sort
    assert_equal ["n100a", "n100b"], @beta.source_notes.map(&:body).sort
    assert_equal [], @gamma.source_notes.map(&:body)
  end

  test "includes batches on the owner's primary_key" do
    seed
    one = mine.includes(:source_profile).order(:title).map {{ |a| [a.title, a.source_profile ? a.source_profile.name : nil] }}
    assert_equal [["Alpha", "p200"], ["Beta", "p100"], ["Gamma", nil]], one
    many = mine.includes(:source_notes).order(:title).map {{ |a| [a.title, a.source_notes.map(&:body).sort] }}
    assert_equal [["Alpha", ["n200"]], ["Beta", ["n100a", "n100b"]], ["Gamma", []]], many
  end

  test "joins ties the tables on the owner's primary_key" do
    seed
    titles = ["Alpha", "Beta", "Gamma"]
    assert_equal ["Alpha", "Beta"], Article.where(title: titles).joins(:source_profile).order(:title).pluck(:title)
    assert_equal ["Alpha", "Beta"], Article.where(title: titles).joins(:source_notes).distinct.order(:title).pluck(:title)
  end

  test "build and create preset the foreign key from the owner's primary_key" do
    seed
    alpha = Article.find(@alpha.id)
    beta = Article.find(@beta.id)
    assert_equal 200, alpha.build_profile_for_test.source_id, "has_one build"
    assert_equal 200, alpha.source_notes.build(body: "built").source_id, "has_many build"
    assert_equal 100, beta.source_notes.create!(body: "created").source_id, "has_many create"
    fresh = Article.create!(title: "Fresh", body: "Body long enough.", source_id: 300)
    assert_equal 300, fresh.create_profile_for_test.source_id, "has_one create"
  end

  test "chained relations off the association keep the owner's primary_key" do
    seed
    alpha = Article.find(@alpha.id)
    beta = Article.find(@beta.id)
    gamma = Article.find(@gamma.id)
    assert_equal ["n200"], alpha.source_notes.order(:body).map(&:body)
    assert_equal ["n100a", "n100b"], beta.source_notes.order(:body).map(&:body)
    assert_equal 1, beta.source_notes.where(body: "n100a").count
    assert_equal 2, beta.source_notes.count
    assert_equal [], gamma.source_notes.order(:body).map(&:body)
  end

  test "dependent destroy removes the rows matched on the primary_key" do
    seed
    @alpha.destroy
    assert_equal ["decoy-for-alpha", "n100a", "n100b", "orphan-note"], SourceNote.order(:body).pluck(:body)
  end
end
"#
    );
    keyed_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n  has_one :source_profile, foreign_key: :source_id, primary_key: :source_id\n  has_many :source_notes, foreign_key: :source_id, primary_key: :source_id, dependent: :destroy\n\n  def build_profile_for_test\n    build_source_profile(name: \"built\")\n  end\n\n  def create_profile_for_test\n    create_source_profile!(name: \"fresh-profile\")\n  end\n",
        )
        .write("test/models/assoc_primary_key_test.rb", &test)
        .run_test("test/models/assoc_primary_key_test.rb")
        .assert_passes();
}

/// A path that cannot honor the key refuses with a diagnostic instead of
/// matching on `id`: a `through:` association whose hop is keyed on a
/// column.
#[test]
fn paths_that_cannot_honor_primary_key_refuse() {
    let (_emitted, errors) = keyed_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n  has_many :source_notes, foreign_key: :source_id, primary_key: :source_id\n  has_many :note_profiles, through: :source_notes, source: :comment\n",
        )
        .write(
            "app/models/source_note.rb",
            "class SourceNote < ApplicationRecord\n  belongs_to :comment, optional: true\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let refused: Vec<&String> = errors.iter().filter(|e| e.contains("primary_key")).collect();
    assert!(
        refused.iter().any(|e| e.contains("Article.note_profiles")),
        "a through: hop keyed on primary_key: must refuse: {errors:?}"
    );
}
