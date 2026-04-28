# Route helpers — module functions returning path strings.
#
# Hand-written for the spinel-blog specimen; in the eventual transpiler
# this module is generated from `config/routes.rb`. One function per
# route × singular/plural × member/collection × format. Each takes
# plain typed args (Integer ids) and returns a String.
#
# Mirrors the routes implied by real-blog's `resources :articles do
# resources :comments` declaration.
module RouteHelpers
  module_function

  # ── articles ─────────────────────────────────────────────────────

  def articles_path
    "/articles"
  end

  def article_path(id)
    "/articles/#{id}"
  end

  def new_article_path
    "/articles/new"
  end

  def edit_article_path(id)
    "/articles/#{id}/edit"
  end

  # ── comments (nested under article) ──────────────────────────────

  def article_comments_path(article_id)
    "/articles/#{article_id}/comments"
  end

  def article_comment_path(article_id, id)
    "/articles/#{article_id}/comments/#{id}"
  end

  def new_article_comment_path(article_id)
    "/articles/#{article_id}/comments/new"
  end

  def edit_article_comment_path(article_id, id)
    "/articles/#{article_id}/comments/#{id}/edit"
  end

  # ── root ─────────────────────────────────────────────────────────

  def root_path
    "/"
  end
end
