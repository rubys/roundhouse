//! Emitted-program regression (kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict). Same harness, plus the
//! declaration lookup the rewrite rests on.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CREATED_UNDER_THE_ARTICLE: (&str, &str) = (
    "    assert_redirected_to article_url(@article)\n  end\n\n  test \"should not create comment with invalid params\" do",
    "    assert_redirected_to article_url(@article)\n    assert_equal @article.id, Comment.last.article_id\n    assert_equal \"A test comment.\", Comment.last.body\n  end\n\n  test \"should not create comment with invalid params\" do",
);

/// `has_many :remarks, class_name: "Comment"` — the parent-ivar
/// rewrite named the class after the association (`Remark`), which
/// does not exist. It builds and finds through the declared class.
#[test]
fn a_nested_build_and_find_through_class_name_use_the_declared_class() {
    let (find, replace) = CREATED_UNDER_THE_ARTICLE;
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, class_name: \"Comment\"",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.remarks.build(comment_params)",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.find(params.expect(:id))",
            "@article.remarks.find(params.expect(:id))",
        )
        .edit("test/controllers/comments_controller_test.rb", find, replace)
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// An ordering scope changes neither what is built nor which row an id
/// finds, so the rewrite still expands it through the declared class.
#[test]
fn a_nested_build_and_find_through_an_ordering_scope_use_the_declared_class() {
    let (find, replace) = CREATED_UNDER_THE_ARTICLE;
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, -> { order(:id) }, class_name: \"Comment\"",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.remarks.build(comment_params)",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.find(params.expect(:id))",
            "@article.remarks.find(params.expect(:id))",
        )
        .edit("test/controllers/comments_controller_test.rb", find, replace)
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// A scope that changes the row set: Rails builds with its `where`
/// values (`commenter: "amy"`), which `<Target>.new(fk => parent.id)`
/// would drop without a word. The rewrite declines it, so the call
/// keeps its source shape instead of building a different record.
#[test]
fn a_nested_build_through_a_row_changing_scope_is_not_expanded() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, -> { where(commenter: \"amy\") }, class_name: \"Comment\"",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.remarks.build(comment_params)",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@article.remarks.build("), "{controller}");
    assert!(!controller.contains("Comment.from_params"), "{controller}");
}

/// A parent ivar not named after its class (`@post` holding an
/// Article): the foreign key is the declared `article_id`, not one
/// derived from the ivar's name (`post_id`).
#[test]
fn a_nested_build_through_an_ivar_not_named_after_the_owner_uses_the_declared_foreign_key() {
    let (find, replace) = CREATED_UNDER_THE_ARTICLE;
    emit_and_run::real_blog()
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.build(comment_params)",
            "    @post = @article\n    @comment = @post.comments.build(comment_params)",
        )
        .edit("test/controllers/comments_controller_test.rb", find, replace)
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// `@post` holding an Article while a `Post` model also declares
/// `comments` (keyed on `post_id`): the declaration comes from the
/// ivar's analyzed class, not from its name, so the comment is still
/// built under `article_id`.
#[test]
fn a_nested_build_through_an_ivar_named_after_another_declarer_uses_the_held_class() {
    let (find, replace) = CREATED_UNDER_THE_ARTICLE;
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.integer \"article_id\", null: false\n",
            "    t.integer \"article_id\", null: false\n    t.integer \"post_id\"\n",
        )
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            "  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\n\n  add_foreign_key \"comments\", \"articles\"",
        )
        .write("app/models/post.rb", "class Post < ApplicationRecord\n  has_many :comments\nend\n")
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.build(comment_params)",
            "    @post = @article\n    @comment = @post.comments.build(comment_params)",
        )
        .edit("test/controllers/comments_controller_test.rb", find, replace)
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// The parent's class picks between declarations; without one, the
/// name alone does not. A subclass finds the declaration it inherits.
#[test]
fn the_declaration_is_resolved_by_the_parent_class_not_the_ivar_name() {
    use roundhouse::ident::{ClassId, Symbol};
    use roundhouse::lower::controller_to_library::params::{AssocTarget, ParamsSpecs};

    let schema = roundhouse::ingest::ingest_schema(
        br#"
ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "type"
  end
  create_table "posts", force: :cascade do |t|
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.integer "post_id"
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ingest schema");
    let models: Vec<_> = [
        ("class Article < ApplicationRecord\n  has_many :comments\nend\n", "app/models/article.rb"),
        ("class Post < ApplicationRecord\n  has_many :comments\nend\n", "app/models/post.rb"),
        ("class Story < Article\nend\n", "app/models/story.rb"),
    ]
    .iter()
    .map(|(src, path)| {
        roundhouse::ingest::ingest_model(src.as_bytes(), path, &schema, &Default::default())
            .expect("ingest model")
            .expect("model recognized")
    })
    .collect();
    let mut specs = ParamsSpecs::default();
    specs.mark_associations(&models);
    let comments = Symbol::from("comments");
    let foreign_key = |owner: &str| {
        match specs.assoc_target(&comments, Some(&ClassId(Symbol::from(owner)))) {
            AssocTarget::Declared { target, foreign_key } => {
                assert_eq!(target.0.as_str(), "Comment");
                Some(foreign_key.as_str().to_string())
            }
            _ => None,
        }
    };
    assert_eq!(foreign_key("Article").as_deref(), Some("article_id"));
    assert_eq!(foreign_key("Post").as_deref(), Some("post_id"));
    assert_eq!(foreign_key("Story").as_deref(), Some("article_id"));
    assert!(matches!(specs.assoc_target(&comments, None), AssocTarget::Unreproducible));
}

/// `has_many :remarks, class_name: "Feedback"` building from
/// `comment_params`: the typed factory exists only on the model the
/// params resource names (`Comment`), so `Feedback.from_params` would
/// call nothing. The rewrite declines and the call keeps its shape.
#[test]
fn a_nested_build_from_another_resources_params_is_not_expanded() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            "  create_table \"feedbacks\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"commenter\"\n    t.text \"body\"\n  end\n\n  add_foreign_key \"comments\", \"articles\"",
        )
        .write("app/models/feedback.rb", "class Feedback < ApplicationRecord\nend\n")
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, class_name: \"Feedback\"",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.remarks.build(comment_params)",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@article.remarks.build("), "{controller}");
    assert!(!controller.contains("Feedback.from_params"), "{controller}");
}

/// `has_many :reviews, class_name: "Comment", foreign_key:
/// :reviewed_article_id`: build presets the declared key, and find
/// answers 404 for a comment the key does not tie to this article, even
/// one whose `article_id` does.
#[test]
fn a_nested_build_and_find_through_a_declared_foreign_key_use_that_key() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.integer \"article_id\", null: false\n",
            "    t.integer \"article_id\"\n    t.integer \"reviewed_article_id\"\n",
        )
        .edit("app/models/comment.rb", "  belongs_to :article\n", "  belongs_to :article, optional: true\n")
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :reviews, class_name: \"Comment\", foreign_key: :reviewed_article_id",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.reviews.build(comment_params)",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.find(params.expect(:id))",
            "@article.reviews.find(params.expect(:id))",
        )
        .edit(
            "test/controllers/comments_controller_test.rb",
            "    assert_redirected_to article_url(@article)\n  end\n\n  test \"should not create comment with invalid params\" do",
            "    assert_redirected_to article_url(@article)\n    assert_equal @article.id, Comment.last.reviewed_article_id\n    assert_nil Comment.last.article_id\n  end\n\n  test \"should not create comment with invalid params\" do",
        )
        .edit(
            "test/controllers/comments_controller_test.rb",
            "  test \"should destroy comment\" do\n    assert_difference(\"Comment.count\", -1) do\n      delete article_comment_url(@article, @comment)\n    end\n    assert_redirected_to article_url(@article)\n  end",
            "  test \"should destroy comment\" do\n    review = Comment.create!(commenter: \"Rev\", body: \"A review.\", reviewed_article_id: @article.id)\n    assert_difference(\"Comment.count\", -1) do\n      delete article_comment_url(@article, review)\n    end\n    assert_redirected_to article_url(@article)\n  end\n\n  test \"should not destroy a comment reviewing another article\" do\n    assert_no_difference(\"Comment.count\") do\n      delete article_comment_url(@article, @comment)\n    end\n    assert_response :not_found\n  end",
        )
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}
