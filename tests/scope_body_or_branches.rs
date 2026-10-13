//! A scope body that builds two relations from its implicit self and
//! combines them (`where(a).or(where(b))`) must give each branch its own
//! relation. Rails' relations are immutable, so the two `where` calls
//! start from the same scope and the result is `a OR b`. The runtime's
//! chain methods mutate in place, and the scope method threads ONE
//! `__rel`: when both branches rooted on it, the receiver's `where`
//! landed on the argument too and the query became `(a AND b) OR
//! (a AND b)`, which finds nothing for a record that matches one side.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn scope_body_or_branches_do_not_share_the_threaded_relation() {
    emit_and_run::real_blog()
        .write(
            "app/models/comment.rb",
            r#"
class Comment < ApplicationRecord
  belongs_to :article
  validates :commenter, presence: true
  validates :body, presence: true

  scope :by_commenter_or_body, ->(text) { where(commenter: text).or(where(body: text)) }
  scope :by_article_or_commenter, ->(id, name) { where(article_id: id).or(by_commenter(name)) }
  scope :by_commenter, ->(name) { where(commenter: name) }
end
"#,
        )
        .run_ruby(
            r#"
article = Article.create!(title: "Owner", body: "A sufficiently long body")
other = Article.create!(title: "Other", body: "Another sufficiently long body")
Comment.create!(article_id: article.id, commenter: "Widget", body: "first")
Comment.create!(article_id: other.id, commenter: "Gadget", body: "Widget")
Comment.create!(article_id: other.id, commenter: "Sprocket", body: "third")

by_name = Comment.by_commenter_or_body("Widget").to_a.map(&:commenter).sort
raise "or over two where branches: #{by_name.inspect}" unless by_name == ["Gadget", "Widget"]

by_scope = Comment.by_article_or_commenter(article.id, "Sprocket").to_a.map(&:commenter).sort
raise "or with a scope branch: #{by_scope.inspect}" unless by_scope == ["Sprocket", "Widget"]

# The scope stays chainable and the receiver relation is not polluted.
chained = Comment.where(article_id: other.id).by_commenter_or_body("Widget").to_a.map(&:commenter)
raise "chained: #{chained.inspect}" unless chained == ["Gadget"]
puts "ok"
"#,
        )
        .assert_passes();
}
