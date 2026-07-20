# In-process spec of the app's full route surface. Run with:  bundle exec ruby test/blog_test.rb
#
# This suite is the behavioral oracle for transpilation experiments
# (rubys/roundhouse#67): a transpiled version of this app must pass it
# unchanged. Requests go through the real middleware stack (config.ru shape),
# so method override, sessions/flash, escaping, and the routing tree's
# interior-node aborts are all exercised as a browser would.

require "tmpdir"
ENV["DATABASE"] = File.join(Dir.mktmpdir("roda-blog-test"), "test.db")

require "minitest/autorun"
require "rack/test"
require_relative "../app"

class BlogTest < Minitest::Test
  include Rack::Test::Methods

  APP = Blog.freeze.app

  def app
    # Rack::MethodOverride sits in config.ru's middleware in spirit; Blog.use
    # already includes it, so Blog.app is the full stack.
    APP
  end

  def setup
    Comment.dataset.delete
    Article.dataset.delete
  end

  def create_article(title: "Hello Roda", body: "A body comfortably over ten characters.", created_at: nil)
    article = Article.new(title: title, body: body)
    article.created_at = created_at if created_at
    article.save
    article
  end

  def create_comment(article, commenter: "Ada", body: "Nice post!", created_at: nil)
    comment = article.add_comment(commenter: commenter, body: body)
    if created_at
      comment.created_at = created_at
      comment.save
    end
    comment
  end

  # --- root ------------------------------------------------------------------

  def test_root_redirects_to_articles
    get "/"
    assert_equal 302, last_response.status
    assert_equal "/articles", URI(last_response.headers["Location"]).path
  end

  # --- articles: collection --------------------------------------------------

  def test_index_lists_articles_newest_first
    create_article(title: "Older", created_at: Time.now - 60)
    create_article(title: "Newer")
    get "/articles"
    assert last_response.ok?
    assert_includes last_response.body, "Older"
    assert_includes last_response.body, "Newer"
    assert_operator last_response.body.index("Newer"), :<, last_response.body.index("Older")
  end

  def test_new_renders_form
    get "/articles/new"
    assert last_response.ok?
    assert_includes last_response.body, %(name="article[title]")
  end

  def test_create_valid_article_redirects_with_notice
    post "/articles", "article" => { "title" => "Created", "body" => "Long enough body text." }
    assert_equal 302, last_response.status
    article = Article.first(title: "Created")
    refute_nil article
    assert_equal "/articles/#{article.id}", URI(last_response.headers["Location"]).path
    follow_redirect!
    assert_includes last_response.body, "Article was successfully created."
  end

  def test_create_invalid_article_rerenders_new_with_errors
    post "/articles", "article" => { "title" => "", "body" => "short" }
    assert last_response.ok?
    assert_includes last_response.body, "error_explanation"
    assert_includes last_response.body, "must be at least 10 characters"
    assert_equal 0, Article.count
  end

  # --- articles: member ------------------------------------------------------

  def test_show_renders_article_and_comments
    article = create_article
    create_comment(article, commenter: "Grace", created_at: Time.now - 60)
    create_comment(article, commenter: "Hopper")
    get "/articles/#{article.id}"
    assert last_response.ok?
    assert_includes last_response.body, article.title
    assert_includes last_response.body, "Grace"
    # one_to_many :comments carries order: Sequel.desc(:created_at) —
    # newest comment renders first.
    assert_operator last_response.body.index("Hopper"), :<, last_response.body.index("Grace")
  end

  def test_show_missing_article_renders_404
    get "/articles/999999"
    assert_equal 404, last_response.status
    assert_includes last_response.body, "404 Not Found"
  end

  def test_edit_renders_form_with_values
    article = create_article(title: "Editable")
    get "/articles/#{article.id}/edit"
    assert last_response.ok?
    assert_includes last_response.body, "Editable"
  end

  def test_update_valid_article_via_method_override
    article = create_article(title: "Before")
    # Browser-shaped request: POST with a hidden _method field, as the form does.
    post "/articles/#{article.id}",
         "_method" => "patch",
         "article" => { "title" => "After", "body" => "Still a valid article body." }
    assert_equal 302, last_response.status
    assert_equal "After", article.reload.title
    follow_redirect!
    assert_includes last_response.body, "Article was successfully updated."
  end

  def test_update_invalid_article_rerenders_edit
    article = create_article(title: "Keep")
    patch "/articles/#{article.id}", "article" => { "title" => "", "body" => "short" }
    assert last_response.ok?
    assert_includes last_response.body, "error_explanation"
    assert_equal "Keep", article.reload.title
  end

  def test_destroy_article_cascades_comments
    article = create_article
    create_comment(article)
    delete "/articles/#{article.id}"
    assert_equal 302, last_response.status
    assert_equal "/articles", URI(last_response.headers["Location"]).path
    assert_equal 0, Article.count
    assert_equal 0, Comment.count
    follow_redirect!
    assert_includes last_response.body, "Article was successfully destroyed."
  end

  def test_member_route_with_non_integer_id_is_404
    get "/articles/garbage"
    assert_equal 404, last_response.status
  end

  # --- comments --------------------------------------------------------------

  def test_create_valid_comment_redirects_with_notice
    article = create_article
    post "/articles/#{article.id}/comments",
         "comment" => { "commenter" => "Ada", "body" => "First!" }
    assert_equal 302, last_response.status
    assert_equal "/articles/#{article.id}", URI(last_response.headers["Location"]).path
    assert_equal 1, article.comments_dataset.count
    follow_redirect!
    assert_includes last_response.body, "Comment was successfully created."
  end

  def test_create_invalid_comment_redirects_with_alert
    article = create_article
    post "/articles/#{article.id}/comments", "comment" => { "commenter" => "", "body" => "" }
    assert_equal 302, last_response.status
    assert_equal 0, Comment.count
    follow_redirect!
    assert_includes last_response.body, "Could not create comment."
  end

  def test_comment_post_requires_exact_path
    # `r.post true` does a path-termination check; extra segments must 404.
    article = create_article
    post "/articles/#{article.id}/comments/garbage",
         "comment" => { "commenter" => "Ada", "body" => "First!" }
    assert_equal 404, last_response.status
    assert_equal 0, Comment.count
  end

  def test_destroy_comment
    article = create_article
    comment = create_comment(article)
    delete "/articles/#{article.id}/comments/#{comment.id}"
    assert_equal 302, last_response.status
    assert_equal 0, Comment.count
    follow_redirect!
    assert_includes last_response.body, "Comment was successfully deleted."
  end

  def test_destroy_missing_comment_is_404
    article = create_article
    delete "/articles/#{article.id}/comments/999999"
    assert_equal 404, last_response.status
  end

  # --- escaping --------------------------------------------------------------

  def test_user_content_is_escaped
    article = create_article(title: %(<script>alert("x")</script>),
                             body: "A perfectly harmless body.")
    get "/articles/#{article.id}"
    assert last_response.ok?
    refute_includes last_response.body, %(<script>alert)
    assert_includes last_response.body, "&lt;script&gt;"
  end
end
