//! `X.create do |x| ... end` inside a model method: lowered to `x =
//! X.new`, the block body, `x.save` (`create!` raises unless it saves)
//! and `x`. The lowering runs after analysis, so it types what it
//! builds; untyped, `X.new` read as a method the class does not have
//! and the emit stopped with an error while `check` was clean.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true\n\n  def log_comment\n    Comment.create do |c|\n      c.commenter = \"Logger\"\n      c.body = \"Logged\"\n      c.article_id = id\n    end\n  end\n\n  def log_blank_comment!\n    Comment.create! do |c|\n      c.article_id = id\n    end\n  end\n",
    )
}

const ASSERTIONS: &str = r##"article = Article.create!(title: "Logged", body: "A body that is long enough.")
comment = article.log_comment
raise "not saved" unless comment.persisted?
stored = Comment.find(comment.id)
raise [stored.commenter, stored.body, stored.article_id].inspect unless [stored.commenter, stored.body, stored.article_id] == ["Logger", "Logged", article.id]
before = Comment.count
begin
  article.log_blank_comment!
  raise "create! saved an invalid comment"
rescue ActiveRecord::RecordInvalid
end
raise "invalid comment stored" unless Comment.count == before
puts "create with a block passed"
"##;

#[test]
fn create_with_a_block_builds_runs_the_block_and_saves() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

// Control: a block whose record fails validation is not saved by `create`.
#[test]
fn create_with_a_block_does_not_save_an_invalid_record() {
    let run = app()
        .edit("app/models/article.rb", "      c.commenter = \"Logger\"\n", "")
        .run_ruby(ASSERTIONS);
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    assert!(!run.success, "a comment without a commenter was saved:\n{}\n{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("not saved") || run.stdout.contains("not saved"), "failed for another reason:\n{}\n{}", run.stdout, run.stderr);
}


// The same through the Spinel build's test harness (it has the database
// `run_spinel`'s library boot does not).
const TEST: &str = r##"require "test_helper"

class ArticleLogTest < ActiveSupport::TestCase
  test "create with a block saves what the block set" do
    article = articles(:one)
    comment = article.log_comment
    assert comment.persisted?
    assert_equal "Logger", Comment.find(comment.id).commenter
  end

  test "create! with a block raises for an invalid record" do
    assert_raises(ActiveRecord::RecordInvalid) { articles(:one).log_blank_comment! }
  end
end
"##;

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn create_with_a_block_passes_in_the_spinel_build() {
    let (emitted, errors) = app().write("test/models/article_log_test.rb", TEST).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(emitted.join("test/article_log_test.rb").exists(), "no emitted test file in {}", emitted.display());
    let output = std::process::Command::new("spin")
        .args(["test", "test/article_log_test.rb"])
        .current_dir(&*emitted)
        .output()
        .expect("spawn spin");
    assert!(
        output.status.success(),
        "spin test failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
