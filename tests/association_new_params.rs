//! Emitted-program regression (kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict). Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// `@article.comments.new(comment_params)` in a nested create — the
/// shape `owner.<has_many>.new(...)` usually takes. `new` is
/// CollectionProxy's alias for `build`, but the controller lowering
/// only knew `build`, so the emitted action called `Array#new` on the
/// reader's result. The early lower pass normalizes the alias to
/// `build`, and the POST saves the comment under the article.
#[test]
fn a_nested_create_with_association_new_saves_under_the_owner() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/comments_controller.rb",
            "@article.comments.build(comment_params)",
            "@article.comments.new(comment_params)",
        )
        .edit(
            "test/controllers/comments_controller_test.rb",
            "    assert_redirected_to article_url(@article)\n  end\n\n  test \"should not create comment with invalid params\" do",
            "    assert_redirected_to article_url(@article)\n    assert_equal @article.id, Comment.last.article_id\n    assert_equal \"A test comment.\", Comment.last.body\n  end\n\n  test \"should not create comment with invalid params\" do",
        )
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}
