//! `assert_difference` / `assert_no_difference` take one expression, an
//! Array of them, or (`assert_difference`) a Hash of expression =>
//! difference; an expression is a String or a lambda
//! (activesupport 8.1.4, oracle `assert_difference.rb`). Each is checked
//! on its own.
//!
//! After a Hash the next argument is the message
//! (`assert_difference({ "Article.count" => 1 }, "message")`).
//!
//! Only a single String was lowered; the other forms reached the test
//! helper, which called `.call` on the Array ("undefined method 'call'
//! for an instance of Array").

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const TEST: &str = r#"
  test "difference forms" do
    attrs = { title: "Forms", body: "A sufficiently long body for validation." }
    assert_difference(["Article.count", "Comment.count"]) do
      a = Article.create!(**attrs)
      Comment.create!(article_id: a.id, commenter: "c", body: "b")
    end
    assert_difference("Article.count" => 1, "Comment.count" => 0) do
      Article.create!(**attrs)
    end
    assert_difference([-> { Article.count }, -> { Comment.count }], 0) do
      Article.count
    end
    assert_no_difference(["Article.count", "Comment.count"]) do
      Article.count
    end
    assert_difference({ "Article.count" => 1 }, "a hash with a message") do
      Article.create!(**attrs)
    end
    assert_difference(-> { Article.count } => 2) do
      Article.create!(**attrs)
      Article.create!(**attrs)
    end
  end

  test "nested assertions keep their own counts" do
    attrs = { title: "Forms", body: "A sufficiently long body for validation." }
    a = Article.create!(**attrs)
    doomed = Article.create!(**attrs)
    assert_difference -> { Article.count }, -1 do
      assert_difference -> { Comment.count }, +1 do
        Comment.create!(article_id: a.id, commenter: "c", body: "b")
        doomed.destroy
      end
    end
    assert_difference("Article.count") do
      assert_no_difference("Comment.count") do
        Article.create!(**attrs)
      end
    end
  end

  test "no expressions runs the block" do
    attrs = { title: "Forms", body: "A sufficiently long body for validation." }
    before = Article.count
    assert_difference([]) do
      Article.create!(**attrs)
    end
    assert_difference({}) do
      Article.create!(**attrs)
    end
    assert_no_difference([]) do
      Article.create!(**attrs)
    end
    assert_equal before + 3, Article.count
  end

  test "difference forms fail per expression" do
    attrs = { title: "Forms", body: "A sufficiently long body for validation." }
    assert_raises(RuntimeError) do
      assert_difference(["Article.count", "Comment.count"]) do
        Article.create!(**attrs)
      end
    end
    assert_raises(RuntimeError) do
      assert_no_difference(["Article.count", "Comment.count"]) do
        Article.create!(**attrs)
      end
    end
    assert_raises(RuntimeError) do
      assert_difference("Article.count" => 1, "Comment.count" => 1) do
        Article.create!(**attrs)
      end
    end
  end

"#;

#[test]
fn every_rails_form_is_checked_on_ruby() {
    emit_and_run::real_blog()
        .edit("test/controllers/articles_controller_test.rb", "  test \"should get new\" do\n", &format!("{TEST}  test \"should get new\" do\n"))
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}
