//! A model method that creates from the permitted params keeps their
//! nested keys. actionpack/activerecord 8.1.4 (oracle `to_attrs.rb`):
//! `Article.publish(article_params)`, whose body is `create(attributes)`,
//! with `comments_attributes` in the permit list creates the article and
//! its comments.
//!
//! Such a call hands the method `article_params.to_attrs`, which listed
//! only scalar fields: the article was created with no comments, while
//! `new(article_params)` and `update` assigned every field and kept them.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const TEST: &str = r#"
  test "a create through a model method keeps the nested comments" do
    post articles_url, params: { article: { title: "Plain", body: "A sufficiently long body for validation.",
      comments_attributes: [{ commenter: "c1", body: "one" }, { commenter: "c2", body: "two" }] } }
    article = Article.last
    assert_equal ["one", "two"], Comment.where(article_id: article.id).order(:id).map(&:body)
  end

"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n  accepts_nested_attributes_for :comments\n\n  def self.publish(attributes)\n    create(attributes)\n  end\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n\n    respond_to do |format|\n      if @article.save\n",
            "    @article = Article.publish(article_params)\n\n    respond_to do |format|\n      if @article.persisted?\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "      params.expect(article: [ :title, :body ])\n",
            "      params.expect(article: [ :title, :body, comments_attributes: [ [ :commenter, :body ] ] ])\n",
        )
        .edit("test/controllers/articles_controller_test.rb", "  test \"should get new\" do\n", &format!("{TEST}  test \"should get new\" do\n"))
}

#[test]
fn a_create_through_a_model_method_keeps_nested_attributes_on_ruby() {
    app().run_test("test/controllers/articles_controller_test.rb").assert_passes();
}
