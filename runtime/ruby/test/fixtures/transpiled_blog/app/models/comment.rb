# Transpiled shape of fixtures/real-blog/app/models/comment.rb.

class Comment < ApplicationRecord
  def self.table_name
    "comments"
  end

  # Schema columns. attr_accessor declares typed-ivar fields and tracks
  # them via schema_column_names for adapter (de)serialization.
  attr_accessor :id, :article_id, :commenter, :body, :created_at, :updated_at

  # --- belongs_to :article ---
  def article
    if @article_id.nil?
      nil
    else
      Article.find(@article_id)
    end
  end

  # --- validates: FK presence (from belongs_to, not optional) + presences ---
  # Note: validates_presence_of(:article) checks that the FK column is
  # set, not that the referenced record exists. Matches Rails's default
  # belongs_to behavior; Juntos's baseline doesn't enforce FK-existence
  # either.
  def validate
    validates_presence_of(:article)
    validates_presence_of(:commenter)
    validates_presence_of(:body)
  end

  # --- broadcasts_to ->(comment) { "article_#{comment.article_id}_comments" }, target: "comments" ---
  # --- after_create_commit { article.broadcast_replace_to("articles") rescue nil } ---
  def after_create_commit
    broadcast_append_to("article_#{article_id}_comments", target: "comments")
    article.broadcast_replace_to("articles") rescue nil
  end

  # --- broadcasts_to (update form) ---
  def after_update_commit
    broadcast_replace_to("article_#{article_id}_comments", target: "comments")
  end

  # --- broadcasts_to (destroy form) ---
  # --- after_destroy_commit { article.broadcast_replace_to("articles") rescue nil } ---
  def after_destroy_commit
    broadcast_remove_to("article_#{article_id}_comments")
    article.broadcast_replace_to("articles") rescue nil
  end
end
