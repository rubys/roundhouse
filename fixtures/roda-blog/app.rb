require_relative "db"
require_relative "models/article"
require_relative "models/comment"
require "roda"
require "rack/method_override"

# A minimal, idiomatic Roda + Sequel blog.
#
# Domain-identical to roundhouse's `real-blog` Rails fixture (Article has_many
# Comment) so the two can be diffed through the same IR / emitters. Where Rails
# and idiomatic Roda/Sequel diverge, this app follows the Roda/Sequel idiom (and
# the README notes the divergence) rather than transliterating Rails.
class Blog < Roda
  # Browser forms can only POST; a hidden `_method` field carries the real verb
  # (PATCH/DELETE). This is the Roda-idiomatic equivalent of Rails' implicit
  # method override.
  use Rack::MethodOverride

  # `escape: true` makes `<%= %>` HTML-escape and `<%== %>` emit raw, so user
  # data is escaped by default and only partial output is explicitly marked raw.
  plugin :render, escape: true, layout: "layout"
  plugin :part                       # part("articles/_form", article: @a) -> render partial w/ locals
  plugin :all_verbs                  # r.patch / r.delete (core Roda ships get/post only)
  plugin :sessions, secret: ENV.fetch("SESSION_SECRET") { "dev-secret-" + "0" * 53 }
  plugin :flash
  plugin :not_found do
    # 404 is already the status when the not_found handler runs; just render.
    view "not_found"
  end

  route do |r|
    # GET / -> canonical /articles. Idiomatic Roda avoids two paths serving the
    # same content (Rails' root+index); redirect to the canonical one instead.
    r.root do
      r.redirect "/articles"
    end

    r.on "articles" do
      # Collection level: /articles
      r.is do
        r.get do
          @articles = Article.eager(:comments).reverse(:created_at).all
          view "articles/index"
        end

        # POST /articles
        r.post do
          @article = Article.new.set_fields(r.params["article"], %w[title body])
          if @article.save
            flash["notice"] = "Article was successfully created."
            r.redirect "/articles/#{@article.id}"
          else
            view "articles/new"
          end
        end
      end

      # GET /articles/new
      r.get "new" do
        @article = Article.new
        view "articles/new"
      end

      # Member level: everything under /articles/:id
      #
      # SEAM (shared interior state + interior abort): @article is loaded once at
      # this interior node and consumed by every sub-branch (show, edit, update,
      # destroy, and the nested comment routes). If it doesn't exist, `next`
      # abandons the whole subtree at the interior node -- the block returns nil,
      # so Roda treats the route as unhandled and the not_found handler renders a
      # 404. This is the idiomatic "return/abort partway down the tree" case that
      # a naive "split each terminal block into a handler" model does not capture;
      # access-control failures use the same interior-node mechanism (with
      # `r.halt` / `r.redirect` instead of `next`).
      r.on Integer do |id|
        next unless @article = Article[id]   # id : Integer, guaranteed by the matcher

        r.is do
          r.get { view "articles/show" }

          # PATCH /articles/:id
          r.patch do
            @article.set_fields(r.params["article"], %w[title body])
            if @article.save
              flash["notice"] = "Article was successfully updated."
              r.redirect "/articles/#{@article.id}"
            else
              view "articles/edit"
            end
          end

          # DELETE /articles/:id
          r.delete do
            @article.destroy
            flash["notice"] = "Article was successfully destroyed."
            r.redirect "/articles"
          end
        end

        # GET /articles/:id/edit
        r.get "edit" do
          view "articles/edit"
        end

        # Nested comments under the already-loaded @article
        r.on "comments" do
          # POST /articles/:id/comments
          #
          # `r.post true` (not bare `r.post`): passing an argument makes Roda also
          # check that the path is fully consumed, so POST /articles/1/comments/x
          # falls through to a 404 instead of matching here.
          r.post true do
            @comment = Comment.new.set_fields(r.params["comment"], %w[commenter body])
            @comment.article = @article
            if @comment.save
              flash["notice"] = "Comment was successfully created."
            else
              flash["alert"] = "Could not create comment."
            end
            r.redirect "/articles/#{@article.id}"
          end

          # DELETE /articles/:id/comments/:comment_id
          r.delete Integer do |comment_id|
            next unless comment = @article.comments_dataset.with_pk(comment_id)
            comment.destroy
            flash["notice"] = "Comment was successfully deleted."
            r.redirect "/articles/#{@article.id}"
          end
        end
      end
    end
  end

  # --- view helpers -----------------------------------------------------------

  def truncate(text, length: 100)
    text = text.to_s
    text.length > length ? "#{text[0, length]}…" : text
  end

  def pluralize(count, singular)
    "#{count} #{count == 1 ? singular : "#{singular}s"}"
  end
end
