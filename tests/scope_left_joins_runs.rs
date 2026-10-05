//! Emitted-program regression for this fix (kept out of tests/emit_and_run.rs
//! so concurrent appends there do not conflict). Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// `left_joins` is Rails' alias for `left_outer_joins`. Only the long
/// spelling was in the scope-body relation-chain table, so a scope that
/// HEADS with `left_joins(…)` stayed a bare send on the class and raised
/// NoMethodError the first time anything called it.
#[test]
fn a_scope_heading_with_left_joins_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  scope :with_comment_rows, -> {\n    left_joins(:comments).select(\"articles.*\").group(\"articles.id\")\n  }\n",
        )
        .run_ruby(r#"
Comment.delete_all
Article.delete_all
a = Article.create!(title: "With", body: "Long enough body")
Article.create!(title: "Without", body: "Long enough body")
Comment.create!(article_id: a.id, commenter: "c", body: "hello there")
Comment.create!(article_id: a.id, commenter: "d", body: "hello again")
rows = Article.with_comment_rows.to_a
raise "left join keeps both articles once each: #{rows.map(&:title).inspect}" unless rows.map(&:title).sort == ["With", "Without"]
puts "left_joins ok"
"#)
        .assert_passes();
}
