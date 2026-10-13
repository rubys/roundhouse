//! `comments.pluck(:id)` in the model that declares `has_many :comments`
//! is rewritten to `comments.map { |__pluck| __pluck.id }`. The receiver
//! is typed there, so the rewritten `map` has to carry `pluck`'s
//! `Array[Integer]` or the strict emit reports `no known method `map`
//! on Array[Comment]` (campfire main's `Room#destroy_later`).
//!
//! The same rewrite must reach a TEST body's association pluck
//! (campfire's `RoomsControllerTest` member-ids assertion) — hook-body
//! walking alone left that site as `Array#pluck`.

use super::emit_and_run;

#[test]
fn a_typed_association_pluck_projects_its_rows() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n\n  def comment_ids_plucked\n    comments.pluck(:id)\n  end\n\n  def commenters_plucked\n    comments.pluck(:commenter).sort\n  end\n",
        )
        .write(
            "test/controllers/plucks_controller_test.rb",
            r#"require "test_helper"

class PlucksControllerTest < ActionDispatch::IntegrationTest
  test "a has_many reader plucks one column" do
    article = Article.create!(title: "Plucked", body: "A body long enough.")
    first = article.comments.create!(commenter: "Zed", body: "First comment")
    second = article.comments.create!(commenter: "Amy", body: "Second comment")
    assert_equal [first.id, second.id].sort, article.comment_ids_plucked.sort
    assert_equal ["Amy", "Zed"], article.commenters_plucked
  end

  test "a test body plucks a has_many reader the same way" do
    article = Article.create!(title: "From test", body: "A body long enough.")
    first = article.comments.create!(commenter: "Zed", body: "First comment")
    second = article.comments.create!(commenter: "Amy", body: "Second comment")
    assert_equal [first.id, second.id].sort, article.comments.pluck(:id).sort
    assert_equal ["Amy", "Zed"], article.comments.pluck(:commenter).sort
  end
end
"#,
        )
        .run_test("test/controllers/plucks_controller_test.rb")
        .assert_passes();
}
