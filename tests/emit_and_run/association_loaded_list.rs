//! `%i[a b].all? { |name| association(name).loaded? }` answers
//! what Rails does: false before the associations load, true once they
//! are preloaded. The block local used to reach `association`, which no
//! model defines, and campfire's `message_fragment_cache_key` raised
//! NoMethodError the first time a keyed collection render called it.

use super::emit_and_run;

#[test]
fn a_symbol_list_of_associations_reports_loaded_as_rails_does() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  def comments_listed_loaded?
    %i[comments].all? { |name| association(name).loaded? }
  end
"#,
        )
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n",
            r#"class Comment < ApplicationRecord
  def article_listed_loaded?
    %i[article].all? { |name| association(name).loaded? }
  end
"#,
        )
        .run_ruby(
            r#"
article = Article.create!(title: "Loaded", body: "long enough body")
comment = Comment.create!(article: article, commenter: "Reader", body: "Comment body")
fresh = Article.find(article.id)
raise "articles start unloaded" if fresh.comments_listed_loaded?
preloaded = ActiveRecord::Relation.new(Article).includes(:comments).to_a.find { |a| a.id == article.id }
raise "includes(:comments) loads them" unless preloaded.comments_listed_loaded?
bare = Comment.find(comment.id)
raise "comments start unloaded" if bare.article_listed_loaded?
bare.article
raise "reading the article loads it" unless bare.article_listed_loaded?
puts "ok"
"#,
        );
    run.assert_passes();
    assert_eq!(run.stdout.trim(), "ok", "stderr:\n{}", run.stderr);
}
