require_relative "application_controller"
require_relative "../models/article"
require_relative "../models/comment"

# Lowered shape of fixtures/real-blog/app/controllers/comments_controller.rb.
#
# `before_action :set_article` (no `only:` filter) lowered to a
# per-action `set_article` call at the top of `process_action`.
# `@article.comments.build(comment_params)` lowered to direct
# `Comment.new(article_id: @article.id, ...)` — same effect.
class CommentsController < ApplicationController
  def process_action(action_name)
    set_article
    case action_name
    when :create  then create
    when :destroy then destroy
    end
  end

  def create
    attrs = comment_params.to_h
    attrs[:article_id] = @article.id
    @comment = Comment.new(attrs)
    if @comment.save
      redirect_to(
        RouteHelpers.article_path(@article.id),
        notice: "Comment was successfully created.",
      )
    else
      redirect_to(
        RouteHelpers.article_path(@article.id),
        alert: "Could not create comment.",
      )
    end
  end

  def destroy
    @comment = Comment.find(@params[:id].to_i)
    # Belongs-to-article check: only allow deletion of comments that
    # belong to the article in the path. Mirrors `@article.comments
    # .find(...)` semantics in real-blog.
    if @comment.article_id != @article.id
      head(:not_found)
      return
    end
    @comment.destroy
    redirect_to(
      RouteHelpers.article_path(@article.id),
      notice: "Comment was successfully deleted.",
    )
  end

  def set_article
    @article = Article.find(@params[:article_id].to_i)
  end

  def comment_params
    @params.require(:comment).permit([:commenter, :body])
  end
end
