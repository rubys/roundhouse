//! Emitted-program regression (kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict). The declaration lookup
//! these runs rest on is pinned in `association_build_lookup`.

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

const COMMENTABLE: &str = "module Commentable\n  extend ActiveSupport::Concern\n\n  included do\n    has_many :comments, dependent: :destroy\n  end\nend\n";

/// `has_many :comments` declared in a concern's `included do`: the
/// splice puts it on the including model, so a parent typed as that
/// model finds it there and the build expands.
#[test]
fn a_nested_build_through_an_association_a_concern_declares_is_expanded() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .write("app/models/concerns/commentable.rb", COMMENTABLE)
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy", "  include Commentable")
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("Comment.from_params"), "{controller}");
    assert!(!controller.contains("@article.comments.build("), "{controller}");
}

/// A concern's `has_many :comments` redeclared after the `include`
/// with another foreign key: the later declaration is the reader the
/// model emits, so build presets that key and find checks it.
#[test]
fn a_nested_build_and_find_through_a_redeclared_association_use_the_later_declaration() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.integer \"article_id\", null: false\n",
            "    t.integer \"article_id\"\n    t.integer \"legacy_article_id\"\n",
        )
        .edit("app/models/comment.rb", "  belongs_to :article\n", "  belongs_to :article, optional: true\n")
        .write("app/models/concerns/commentable.rb", COMMENTABLE)
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  include Commentable\n  has_many :comments, foreign_key: :legacy_article_id",
        )
        .edit(
            "test/controllers/comments_controller_test.rb",
            "    assert_redirected_to article_url(@article)\n  end\n\n  test \"should not create comment with invalid params\" do",
            "    assert_redirected_to article_url(@article)\n    assert_equal @article.id, Comment.last.legacy_article_id\n    assert_nil Comment.last.article_id\n  end\n\n  test \"should not create comment with invalid params\" do",
        )
        .edit(
            "test/controllers/comments_controller_test.rb",
            "  test \"should destroy comment\" do\n    assert_difference(\"Comment.count\", -1) do\n      delete article_comment_url(@article, @comment)\n    end\n    assert_redirected_to article_url(@article)\n  end",
            "  test \"should destroy comment\" do\n    legacy = Comment.create!(commenter: \"Leg\", body: \"A legacy comment.\", legacy_article_id: @article.id)\n    assert_difference(\"Comment.count\", -1) do\n      delete article_comment_url(@article, legacy)\n    end\n    assert_redirected_to article_url(@article)\n  end\n\n  test \"should not destroy a comment keyed by the earlier declaration\" do\n    assert_no_difference(\"Comment.count\") do\n      delete article_comment_url(@article, @comment)\n    end\n    assert_response :not_found\n  end",
        )
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// The later declaration adds a `where` scope Rails builds with, so
/// the rewrite declines it rather than expand the concern's plain one.
#[test]
fn a_nested_build_and_find_through_a_redeclared_row_changing_association_are_not_expanded() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .write("app/models/concerns/commentable.rb", COMMENTABLE)
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  include Commentable\n  has_many :comments, -> { where(commenter: \"amy\") }",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@article.comments.build("), "{controller}");
    assert!(controller.contains("@article.comments.find("), "{controller}");
    assert!(!controller.contains("Comment.from_params"), "{controller}");
}

/// `@tag.comments` where only Article declares `comments`: Tag has no
/// such association, so neither the rewrite nor the association seed
/// builds or finds a Comment by Article's `article_id`.
#[test]
fn a_nested_build_and_find_on_a_model_without_the_association_do_not_borrow_another_models_key() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            "  create_table \"tags\", force: :cascade do |t|\n    t.string \"name\"\n  end\n\n  add_foreign_key \"comments\", \"articles\"",
        )
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\nend\n")
        .edit(
            "app/controllers/comments_controller.rb",
            "      @article = Article.find(params.expect(:article_id))",
            "      @article = Article.find(params.expect(:article_id))\n      @tag = Tag.find(params.expect(:article_id))",
        )
        .edit("app/controllers/comments_controller.rb", "@article.comments.build(comment_params)", "@tag.comments.build(comment_params)")
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.find(params.expect(:id))",
            "@tag.comments.find(params.expect(:id))",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@tag.comments.build("), "{controller}");
    assert!(controller.contains("@tag.comments.find("), "{controller}");
    assert!(
        !controller.lines().any(|l| l.contains("article_id") && l.contains("@tag.id")),
        "{controller}"
    );
}

/// `@owner` holding a Tag or an Article: only the Article has
/// `comments`, so neither the rewrite nor the association seed builds
/// or finds a Comment under the Tag's id as its `article_id`. The
/// kwargs build and the `find` reach the seed, which the rewrite leaves
/// them to.
#[test]
fn a_nested_build_on_a_union_of_models_does_not_borrow_one_arms_key() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            "  create_table \"tags\", force: :cascade do |t|\n    t.string \"name\"\n  end\n\n  add_foreign_key \"comments\", \"articles\"",
        )
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\nend\n")
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.build(comment_params)",
            "    @owner = params[:tag] ? Tag.find(1) : @article\n    @comment = @owner.comments.build(comment_params)\n    @draft = @owner.comments.build(commenter: \"amy\")",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.find(params.expect(:id))",
            "    @owner = params[:tag] ? Tag.find(1) : @article\n    @comment = @owner.comments.find(params.expect(:id))",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@owner.comments.build(self.comment_params)"), "{controller}");
    assert!(controller.contains("@owner.comments.build(commenter: \"amy\")"), "{controller}");
    assert!(controller.contains("@owner.comments.find("), "{controller}");
    assert!(
        !controller.lines().any(|l| l.contains("article_id") && l.contains("@owner.id") || l.contains("article: @owner")),
        "{controller}"
    );
}

/// Post's `comments` extension defines `count`; Article's is Rails'
/// own. `@article.comments.count` counts Article's comments rather than
/// calling a `comments_count` Article does not have.
#[test]
fn a_typed_owner_ignores_another_models_extension() {
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
        .write(
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  has_many :comments do\n    def count\n      0\n    end\n  end\nend\n",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.build(comment_params)\n",
            "    @comment = @article.comments.build(comment_params)\n    @comment_count = @article.comments.count\n",
        )
        .edit("test/controllers/comments_controller_test.rb", find, replace)
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// An extension defining `find` is the method `@article.remarks.find`
/// runs: that call reaches it (flattened to `remarks_find`), and
/// `build`, which the extension leaves alone, still expands.
#[test]
fn a_nested_find_through_an_extension_defining_find_reaches_the_extension() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, class_name: \"Comment\" do\n    def find(id)\n      Comment.find_by(id: id, commenter: \"amy\")\n    end\n  end",
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
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@article.remarks_find("), "{controller}");
    assert!(!controller.contains("Comment.find("), "{controller}");
    assert!(controller.contains("Comment.from_params"), "{controller}");
}

/// An extension defining only an unrelated method leaves `build` and
/// `find` as Rails' own, so both still expand through the declaration.
#[test]
fn a_nested_build_and_find_through_an_extension_with_other_methods_use_the_declared_class() {
    let (find, replace) = CREATED_UNDER_THE_ARTICLE;
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, class_name: \"Comment\" do\n    def by(commenter)\n      where(commenter: commenter)\n    end\n  end",
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

/// CollectionProxy's `new` is `build` under another name, unless an
/// extension defines `new`: then `@article.remarks.new` runs it, and
/// is neither turned into `build` nor expanded.
#[test]
fn a_nested_new_through_an_extension_defining_new_reaches_the_extension() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, class_name: \"Comment\" do\n    def new(attributes = {})\n      Comment.new(commenter: \"amy\")\n    end\n  end",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.remarks.new(comment_params)",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@article.remarks_new("), "{controller}");
    assert!(!controller.contains("Comment.from_params"), "{controller}");
}

/// The extension defining `new` comes with the later declaration of
/// `remarks`, the one whose reader runs, so `new` still reaches it.
#[test]
fn a_nested_new_through_a_redeclared_extension_defining_new_reaches_the_extension() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, class_name: \"Comment\"\n  has_many :remarks, class_name: \"Comment\" do\n    def new(attributes = {})\n      Comment.new(commenter: \"amy\")\n    end\n  end",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.remarks.new(comment_params)",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@article.remarks_new("), "{controller}");
    assert!(!controller.contains("Comment.from_params"), "{controller}");
}

/// `has_many :remarks, -> { order(created_at: :desc) }`: Rails' `find_by`
/// takes the first match in that order, which `Comment.find_by(…,
/// article_id:)` would not, so the rewrite declines `find_by` while
/// `build` and `find` still expand.
#[test]
fn a_nested_find_by_through_an_ordering_scope_is_not_expanded() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, -> { order(created_at: :desc) }, class_name: \"Comment\"",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.remarks.build(comment_params)",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.find(params.expect(:id))",
            "    @comment = @article.remarks.find(params.expect(:id))\n    @latest = @article.remarks.find_by(commenter: @comment.commenter)",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("@article.remarks.find_by("), "{controller}");
    assert!(!controller.contains("Comment.find_by("), "{controller}");
    assert!(controller.contains("Comment.from_params"), "{controller}");
    assert!(controller.contains("Comment.find("), "{controller}");
}

/// `@story` of no analyzed type, while Story and Article both declare
/// `comments`: the name says Story, as it does for the association seed,
/// so the typed-params build expands under `story_id` rather than being
/// left on the reader.
#[test]
fn a_nested_build_on_an_untyped_parent_named_after_a_declarer_uses_its_key() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.integer \"article_id\", null: false\n",
            "    t.integer \"article_id\"\n    t.integer \"story_id\"\n",
        )
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            "  create_table \"stories\", force: :cascade do |t|\n    t.string \"title\"\n  end\n\n  add_foreign_key \"comments\", \"articles\"",
        )
        .write("app/models/story.rb", "class Story < ApplicationRecord\n  has_many :comments\nend\n")
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@story.comments.build(comment_params)",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("Comment.from_params"), "{controller}");
    assert!(controller.contains("rh_build_owner_0 = @story"), "{controller}");
    assert!(controller.contains("rh_build_record_0.story_id = rh_build_owner_0.id"), "{controller}");
    assert!(!controller.contains("@story.comments.build("), "{controller}");
}

/// An extension defining `build` does not change `new`: Rails binds the
/// proxy's `new` to its own `build` where the alias is defined, so
/// `@article.remarks.new(comment_params)` builds a Comment of the
/// declaration with those params, not what the extension's `build`
/// makes.
#[test]
fn a_nested_new_through_an_extension_defining_build_builds_through_the_declaration() {
    let (find, replace) = CREATED_UNDER_THE_ARTICLE;
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :remarks, class_name: \"Comment\" do\n    def build(attributes = {})\n      Comment.new(commenter: \"amy\")\n    end\n  end",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.remarks.new(comment_params)",
        )
        .edit("test/controllers/comments_controller_test.rb", find, replace)
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// `Article < Publication`, an abstract base declaring `comments`:
/// `@article.comments` is the association Article inherits, so its `new`
/// is `build` there too, and the seed presets the declared key.
#[test]
fn a_new_through_an_inherited_association_is_build() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .write(
            "app/models/publication.rb",
            "class Publication < ApplicationRecord\n  self.abstract_class = true\n  has_many :comments, foreign_key: :article_id, dependent: :destroy\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < Publication\n",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.build(comment_params)\n",
            "    @comment = @article.comments.build(comment_params)\n    draft = @article.comments.new(commenter: \"amy\")\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(
        controller.contains("rh_build_owner_1 = @article")
            && controller.contains("draft = Comment.new({ commenter: \"amy\" }.merge(article_id: rh_build_owner_1.id))"),
        "{controller}"
    );
}

/// `create` and `find_by` through `class_name:` and `foreign_key:`:
/// create presets the declared key, and `find_by` finds nothing for a
/// comment the key does not tie to this article, even one whose
/// `article_id` does.
#[test]
fn a_nested_create_and_find_by_through_a_declared_foreign_key_use_that_key() {
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
            "    @comment = @article.comments.build(comment_params)\n    if @comment.save",
            "    @comment = @article.reviews.create(comment_params)\n    if @comment.persisted?",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.find(params.expect(:id))\n    @comment.destroy\n    redirect_to @article, notice: \"Comment was successfully deleted.\"",
            "    @comment = @article.reviews.find_by(id: params.expect(:id))\n    if @comment\n      @comment.destroy\n      redirect_to @article, notice: \"Comment was successfully deleted.\"\n    else\n      head :not_found\n    end",
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

/// `create!` through the same declaration: the typed factory of the
/// declared class, saved with the declared key preset.
#[test]
fn a_nested_create_bang_through_a_declared_foreign_key_uses_that_key() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.integer \"article_id\", null: false\n",
            "    t.integer \"article_id\"\n    t.integer \"reviewed_article_id\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            "  has_many :comments, dependent: :destroy\n  has_many :reviews, class_name: \"Comment\", foreign_key: :reviewed_article_id",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.reviews.create!(comment_params)",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/comments_controller.rb"))
        .expect("emitted controller");
    assert!(controller.contains("Comment.from_params"), "{controller}");
    assert!(controller.lines().any(|l| l.contains("reviewed_article_id") && l.contains("@article.id")), "{controller}");
    assert!(!controller.contains("@article.reviews.create!("), "{controller}");
}

/// Campfire's `@membership, @room = RecordCache.fetch(…) { [membership,
/// membership.room] }` types `@room` as `Membership | Room`, as this
/// types `@article` `Comment | Article`. Only the Article arm has
/// `comments`, and the ivar is named after it, so a concern's class
/// method called through the association (campfire's
/// `@room.messages.create_with_attachment!(message_params)`) runs
/// against the article's comments: it takes the relation, the foreign
/// key is preset, and the params arrive as an attribute hash. The
/// `find` in `destroy` is checked against the same key.
#[test]
fn a_class_method_through_an_association_on_a_union_typed_owner_named_after_its_arm_takes_the_scope() {
    let (find, replace) = CREATED_UNDER_THE_ARTICLE;
    emit_and_run::real_blog()
        .write(
            "app/models/comment/noting.rb",
            "module Comment::Noting\n  extend ActiveSupport::Concern\n\n  module ClassMethods\n    def create_noted!(attributes)\n      create!(attributes)\n    end\n  end\nend\n",
        )
        .edit("app/models/comment.rb", "class Comment < ApplicationRecord\n", "class Comment < ApplicationRecord\n  include Noting\n\n")
        .edit(
            "app/controllers/comments_controller.rb",
            "      @article = Article.find(params.expect(:article_id))",
            "      @first_comment, @article = [ Comment.first, Article.find(params.expect(:article_id)) ]",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment = @article.comments.build(comment_params)\n    if @comment.save\n      redirect_to @article, notice: \"Comment was successfully created.\"\n    else\n      redirect_to @article, alert: \"Could not create comment.\"\n    end\n  end",
            "    @comment = @article.comments.create_noted!(comment_params)\n    redirect_to @article, notice: \"Comment was successfully created.\"\n  rescue ActiveRecord::RecordInvalid\n    redirect_to @article, alert: \"Could not create comment.\"\n  end",
        )
        .edit("test/controllers/comments_controller_test.rb", find, replace)
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// The same union-typed `@article` reading `comments` through a scope
/// and through a class method that queries: each reads the article's
/// comments alone. Destroying article one's only comment leaves it
/// none while article two keeps its own.
#[test]
fn a_scope_and_a_query_class_method_through_an_association_on_a_union_typed_owner_read_its_rows() {
    emit_and_run::real_blog()
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n",
            "class Comment < ApplicationRecord\n  scope :recent, -> { order(created_at: :desc) }\n\n  def self.tally\n    count\n  end\n\n",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "      @article = Article.find(params.expect(:article_id))",
            "      @first_comment, @article = [ Comment.first, Article.find(params.expect(:article_id)) ]",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "    @comment.destroy\n    redirect_to @article, notice: \"Comment was successfully deleted.\"",
            "    @comment.destroy\n    if @article.comments.tally == 0 && @article.comments.recent.first.nil?\n      redirect_to @article, notice: \"Comment was successfully deleted.\"\n    else\n      head :conflict\n    end",
        )
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}
