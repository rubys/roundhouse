//! `include Discard::Model` (the `discard` gem) on real-blog: the scopes and
//! predicates it adds, and the write half with its `after_discard` hook.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "Gemfile.lock",
            "    debug (1.11.1)\n",
            "    discard (2.0.0)\n      activerecord (>= 5.0, < 9.0)\n    debug (1.11.1)\n",
        )
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|\n",
            "create_table \"articles\", force: :cascade do |t|\n    t.datetime \"discarded_at\"\n",
        )
        .edit(
            "db/schema.rb",
            "create_table \"comments\", force: :cascade do |t|\n",
            "create_table \"comments\", force: :cascade do |t|\n    t.datetime \"discarded_at\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  include Discard::Model\n  has_many :comments, dependent: :destroy\n\
             \x20 after_discard :note_discarded, :note_again\n  after_undiscard :note_undiscarded\n\n\
             \x20 def notes\n    @notes ||= []\n  end\n\n\
             \x20 def note_discarded\n    notes << \"discarded:#{discarded?}\"\n  end\n\n\
             \x20 def note_again\n    notes << \"again\"\n  end\n\n\
             \x20 def note_undiscarded\n    notes << \"undiscarded:#{discarded?}\"\n  end\n",
        )
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article\n",
            "  include Discard::Model\n  belongs_to :article\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "Article.includes(:comments).order(created_at: :desc)",
            "Article.kept.includes(:comments).order(created_at: :desc)",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "@article = Article.find(params.expect(:id))",
            "@article = Article.with_discarded.find(params.expect(:id))",
        )
}

const TEST: &str = r#"require "test_helper"

class DiscardsControllerTest < ActionDispatch::IntegrationTest
  test "kept and discarded partition the table" do
    gone = Article.create!(title: "Hidden away", body: "This one is discarded.")
    gone.update_attribute(:discarded_at, Time.current)
    kept_titles = Article.kept.map(&:title).sort
    assert_equal ["Getting Started with Rails", "Understanding MVC Architecture"], kept_titles
    assert_equal ["Hidden away"], Article.discarded.map(&:title)
    assert_equal 3, Article.with_discarded.count
    assert_equal 2, Article.undiscarded.count
    assert_equal 2, Article.kept.count
  end

  test "predicates answer per record" do
    assert articles(:one).kept?
    assert articles(:one).undiscarded?
    assert_not articles(:one).discarded?
  end

  test "with_discarded undoes a kept chain but keeps other conditions" do
    gone = Article.create!(title: "Hidden away", body: "This one is discarded.")
    gone.update_attribute(:discarded_at, Time.current)
    assert_equal 3, Article.kept.with_discarded.count
    assert_equal ["Hidden away"], Article.kept.with_discarded.where(title: "Hidden away").map(&:title)
    assert_equal 0, Article.kept.with_discarded.where(title: "No such").count
  end

  test "association relations take the scopes" do
    article = articles(:one)
    hidden = article.comments.create!(commenter: "Eve", body: "Hidden comment")
    hidden.update_attribute(:discarded_at, Time.current)
    assert_equal ["Alice"], article.comments.kept.map(&:commenter)
    assert_equal ["Eve"], article.comments.discarded.map(&:commenter)
    assert_equal ["Alice", "Eve"], article.comments.with_discarded.map(&:commenter).sort
  end

  test "kept is NULL-tested, not equality-tested" do
    sql = Article.kept.to_sql
    assert_includes sql, "discarded_at IS NULL"
    assert_not_includes sql, "= NULL"
    assert_includes Article.discarded.to_sql, "NOT"
  end

  test "discard and undiscard flip the column and run the after callbacks in order" do
    article = articles(:one)
    assert article.discard
    assert article.discarded?
    assert_not_nil Article.with_discarded.find(article.id).discarded_at
    assert_equal ["discarded:true", "again"], article.notes
    assert_not article.discard
    assert_equal ["discarded:true", "again"], article.notes
    assert_equal 1, Article.kept.count

    assert article.undiscard
    assert article.kept?
    assert_nil Article.find(article.id).discarded_at
    assert_equal ["discarded:true", "again", "undiscarded:false"], article.notes
    assert_not article.undiscard
    assert_equal 2, Article.kept.count
  end

  test "bang forms raise when there is nothing to do" do
    article = articles(:two)
    assert_equal true, article.discard!
    assert_raises(ActiveRecord::RecordNotSaved) { article.discard! }
    assert_equal true, article.undiscard!
    assert_raises(ActiveRecord::RecordNotSaved) { article.undiscard! }
  end

  test "index lists only kept articles and show reaches a discarded one" do
    articles(:two).discard
    get articles_url
    assert_response :success
    assert_includes response.body, "Getting Started with Rails"
    assert_not_includes response.body, "Understanding MVC Architecture"
    get article_url(articles(:two))
    assert_response :success
  end
end
"#;

#[test]
fn discard_model_runs_in_emitted_ruby() {
    let run = app().write("test/controllers/discards_controller_test.rb", TEST).run_test("test/controllers/discards_controller_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("8 tests passed"), "{}", run.stdout);
}

#[test]
fn discard_model_runs_the_controller_suite_too() {
    app().run_test("test/controllers/articles_controller_test.rb").assert_passes();
}
