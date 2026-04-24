# Transpiled shape of fixtures/real-blog/app/models/comment.rb.
#
# Expansion applied:
#   - attributes: explicit getter/setter per schema column
#   - belongs_to :article: explicit `article` reader + FK-existence
#     check inlined into validate
#   - validates: explicit `validate` method
#   - broadcasts_to ->(comment) { "article_#{comment.article_id}_comments" },
#     target: "comments":
#     inlined into after_create_commit / after_update_commit / after_destroy_commit
#     — the lambda body becomes a string interpolation with self.
#   - after_create_commit { article.broadcast_replace_to("articles") rescue nil }:
#     composed with the broadcasts_to expansion. Both effects live in the
#     same after_create_commit override; the explicit-block form goes after
#     the broadcasts_to derivation.

class Comment < ApplicationRecord
  def self.table_name
    "comments"
  end

  # --- Attributes (from schema) ---
  def id;             @attributes[:id];             end
  def id=(v);         @attributes[:id] = v;         end
  def article_id;     @attributes[:article_id];     end
  def article_id=(v); @attributes[:article_id] = v; end
  def commenter;      @attributes[:commenter];      end
  def commenter=(v);  @attributes[:commenter] = v;  end
  def body;           @attributes[:body];           end
  def body=(v);       @attributes[:body] = v;       end
  def created_at;     @attributes[:created_at];     end
  def created_at=(v); @attributes[:created_at] = v; end
  def updated_at;     @attributes[:updated_at];     end
  def updated_at=(v); @attributes[:updated_at] = v; end

  # --- belongs_to :article ---
  def article
    return nil if @attributes[:article_id].nil?
    Article.find(@attributes[:article_id])
  rescue ActiveRecord::RecordNotFound
    nil
  end

  # --- validates: FK existence (from belongs_to, not optional) + presences ---
  def validate
    if @attributes[:article_id].nil?
      errors << "article can't be blank"
    elsif !Article.exists?(@attributes[:article_id])
      errors << "Article must exist"
    end
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
