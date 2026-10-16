//! `delegate` declared in an included model concern forwards through a
//! generated Active Record association in emitted Ruby.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\nend\n",
            "  validates :body, presence: true, length: { minimum: 10 }\n\n  def +(other)\n    other\n  end\nend\n",
        )
        .write(
            "app/models/concerns/article_title.rb",
            r#"module ArticleTitle
  extend ActiveSupport::Concern

  included do
    delegate :title, to: :article
  end
end
"#,
        )
        .write(
            "app/models/comment.rb",
            r#"class Comment < ApplicationRecord
  belongs_to :article
  delegate :body, to: :article, prefix: true
  delegate :title=, to: :article
  delegate :+, to: :article
  include ArticleTitle

  def plus_via_delegate(other)
    self + other
  end
end
"#,
        )
}

pub const ASSERTIONS: &str = r#"
article = Article.create!(title: "Association title", body: "long enough body")
comment = Comment.create!(article: article, commenter: "Reader", body: "Nice")
loaded = Comment.find(comment.id)
raise "delegation lost" unless loaded.title == "Association title"
raise "direct model delegation lost" unless loaded.article_body == "long enough body"
loaded.title = "Updated title"
raise "delegated writer failed" unless loaded.article.save!
raise "delegated writer did not persist" unless Article.find(article.id).title == "Updated title"
raise "delegated operator failed" unless loaded + "operator" == "operator"
raise "delegated operator call inside a model failed" unless loaded.plus_via_delegate("forwarded") == "forwarded"
puts "model concern delegate passed"
"#;
