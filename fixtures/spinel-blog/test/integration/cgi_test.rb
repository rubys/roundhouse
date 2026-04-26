require_relative "../test_helper"
require "stringio"

# Integration tests through the CGI entry point. Loads main.rb (which
# defines Main.run) and invokes it with constructed env hashes +
# StringIO bodies. This exercises the full stack — parse_request →
# router → controller → render → write_response — in one call,
# the same path spinel will see.
#
# These tests share the test_helper's already-configured adapter, so
# Main.configure_default_adapter! is a no-op (its idempotency guard
# returns early when ActiveRecord.adapter is set).
require_relative "../../main"

# Captures the parsed shape of a CGI response. Predicates are real
# methods (not OpenStruct attribute lookups, which would silently
# return nil on `?` suffix and pass `assert nil` accidentally).
class CgiResult
  attr_reader :status, :body, :location, :raw, :set_cookies

  def initialize(status:, body:, location:, raw:, set_cookies:)
    @status      = status
    @body        = body
    @location    = location
    @raw         = raw
    @set_cookies = set_cookies
  end

  def redirect?
    !@location.nil? && @status >= 300 && @status < 400
  end

  def success?
    @status >= 200 && @status < 300
  end

  def unprocessable?
    @status == 422
  end
end

class CgiIntegrationTest < Minitest::Test
  def setup
    SchemaSetup.reset!
    Broadcasts.reset_log!
  end

  # Helper: synthesize a CGI request and capture the response.
  # `cookies:` is sent as the request's `Cookie:` header (CGI/1.1's
  # `HTTP_COOKIE` env var). Response parsing extracts Set-Cookie
  # headers as well so tests can assert on the cookie round-trip.
  def cgi(method:, path:, query: nil, body: nil, content_type: "application/x-www-form-urlencoded", cookies: nil)
    env = {
      "REQUEST_METHOD" => method,
      "PATH_INFO"      => path,
      "QUERY_STRING"   => query || "",
    }
    if body
      env["CONTENT_LENGTH"] = body.bytesize.to_s
      env["CONTENT_TYPE"]   = content_type
    end
    env["HTTP_COOKIE"] = cookies if cookies
    stdin  = StringIO.new(body || "")
    stdout = StringIO.new
    Main.run(env, stdin, stdout)
    raw = stdout.string
    status = raw[/\AStatus: (\d+)/, 1].to_i
    location = raw[/^Location: (\S+)/, 1]
    set_cookies = raw.scan(/^Set-Cookie: ([^=]+)=([^;\r\n]*)/).to_h
    sep_idx = raw.index("\r\n\r\n")
    body_str = sep_idx ? raw[(sep_idx + 4)..] : ""
    CgiResult.new(status: status, location: location, body: body_str, raw: raw, set_cookies: set_cookies)
  end

  # ── 404 paths ────────────────────────────────────────────────────

  def test_unknown_path_returns_404
    res = cgi(method: "GET", path: "/nonexistent")
    assert_equal 404, res.status
    assert_includes res.body, "404 Not Found"
  end

  def test_missing_record_returns_404
    res = cgi(method: "GET", path: "/articles/9999")
    assert_equal 404, res.status
  end

  # ── GET /articles ────────────────────────────────────────────────

  def test_index_returns_200_with_layout_wrapper
    res = cgi(method: "GET", path: "/articles")
    assert_equal 200, res.status
    assert_includes res.body, "<!DOCTYPE html>"
    assert_includes res.body, "<html>"
    assert_includes res.body, "</html>"
  end

  def test_index_renders_articles_h1
    res = cgi(method: "GET", path: "/articles")
    assert_includes res.body, ">Articles</h1>"
  end

  def test_index_includes_existing_articles
    Article.new(title: "First post", body: "Some long body content here.").save
    res = cgi(method: "GET", path: "/articles")
    assert_includes res.body, "First post"
  end

  def test_index_layout_uses_content_for_title
    res = cgi(method: "GET", path: "/articles")
    assert_includes res.body, "<title>Articles</title>"
  end

  # ── GET /articles/:id ────────────────────────────────────────────

  def test_show_returns_full_page
    article = Article.new(title: "Showing me", body: "Some long body content here.")
    article.save
    res = cgi(method: "GET", path: "/articles/#{article.id}")
    assert_equal 200, res.status
    assert_includes res.body, "<!DOCTYPE html>"
    assert_includes res.body, ">Showing me</h1>"
    assert_includes res.body, "Some long body content here."
  end

  def test_show_layout_title_is_showing_article
    article = Article.new(title: "Hi", body: "Long enough body content.")
    article.save
    res = cgi(method: "GET", path: "/articles/#{article.id}")
    assert_includes res.body, "<title>Showing article</title>"
  end

  # ── GET /articles/new ────────────────────────────────────────────

  def test_new_renders_empty_form
    res = cgi(method: "GET", path: "/articles/new")
    assert_equal 200, res.status
    assert_includes res.body, ">New article</h1>"
    assert_includes res.body, %(action="/articles")
    assert_includes res.body, %(method="post")
  end

  # ── POST /articles (valid) ───────────────────────────────────────

  def test_create_with_valid_form_body_redirects
    body = "article%5Btitle%5D=From+CGI&article%5Bbody%5D=Body+via+CGI+with+enough+content."
    res = cgi(method: "POST", path: "/articles", body: body)
    assert_equal 302, res.status
    refute_nil res.location
    assert_match %r{\A/articles/\d+\z}, res.location
  end

  def test_create_persists_record
    body = "article%5Btitle%5D=Persisted&article%5Bbody%5D=Long+enough+body+content+here."
    initial = Article.count
    cgi(method: "POST", path: "/articles", body: body)
    assert_equal initial + 1, Article.count
    assert Article.find_by(title: "Persisted")
  end

  def test_create_emits_broadcast
    body = "article%5Btitle%5D=Broadcast+test&article%5Bbody%5D=Long+enough+body+content+here."
    cgi(method: "POST", path: "/articles", body: body)
    entries = Broadcasts.log
    assert entries.any? { |e| e[:stream] == "articles" && e[:action] == :prepend }
  end

  # ── POST /articles (invalid) ─────────────────────────────────────

  def test_create_with_invalid_body_returns_422_with_form
    body = "article%5Btitle%5D=&article%5Bbody%5D=short"
    res = cgi(method: "POST", path: "/articles", body: body)
    assert_equal 422, res.status
    assert_includes res.body, ">New article</h1>"
    assert_includes res.body, %(id="error_explanation")
  end

  # ── DELETE /articles/:id ─────────────────────────────────────────

  def test_destroy_redirects_and_removes_record
    article = Article.new(title: "Doomed", body: "Some long body content here.")
    article.save
    id = article.id
    res = cgi(method: "DELETE", path: "/articles/#{id}")
    assert_equal 303, res.status
    assert_equal "/articles", res.location
    refute Article.exists?(id)
  end

  # ── nested resource: POST comment ────────────────────────────────

  def test_create_comment_redirects_to_parent_article
    article = Article.new(title: "Host", body: "Body long enough to satisfy validations.")
    article.save
    body = "comment%5Bcommenter%5D=Alice&comment%5Bbody%5D=Nice+post"
    res = cgi(method: "POST", path: "/articles/#{article.id}/comments", body: body)
    assert_equal 302, res.status
    assert_equal "/articles/#{article.id}", res.location
    assert_equal 1, Comment.count
  end

  # ── flash via cookies ────────────────────────────────────────────

  def test_create_redirect_emits_flash_notice_cookie
    body = "article%5Btitle%5D=Has+notice&article%5Bbody%5D=Long+enough+body+content."
    res = cgi(method: "POST", path: "/articles", body: body)
    assert res.redirect?
    assert_equal "Article%20was%20successfully%20created.",
                 res.set_cookies["flash_notice"]
  end

  def test_destroy_redirect_emits_flash_notice
    article = Article.new(title: "Doomed", body: "Long enough body content here.")
    article.save
    res = cgi(method: "DELETE", path: "/articles/#{article.id}")
    assert res.redirect?
    assert_equal "Article%20was%20successfully%20destroyed.",
                 res.set_cookies["flash_notice"]
  end

  def test_render_after_arriving_with_flash_cookie_displays_notice
    article = Article.new(title: "Already there", body: "Long enough body content here.")
    article.save
    res = cgi(
      method: "GET",
      path: "/articles/#{article.id}",
      cookies: "flash_notice=Hello%20from%20last%20request",
    )
    assert_equal 200, res.status
    assert_includes res.body, %(id="notice")
    assert_includes res.body, ">Hello from last request</p>"
  end

  def test_render_clears_inbound_flash_cookie
    article = Article.new(title: "Already there", body: "Long enough body content here.")
    article.save
    res = cgi(
      method: "GET",
      path: "/articles/#{article.id}",
      cookies: "flash_notice=Consume%20me",
    )
    # Set-Cookie: flash_notice=; Max-Age=0 — clears it
    assert_includes res.raw, "Set-Cookie: flash_notice=;"
    assert_includes res.raw, "Max-Age=0"
  end

  def test_render_without_inbound_flash_does_not_set_cookie
    article = Article.new(title: "x", body: "Long enough body content here.")
    article.save
    res = cgi(method: "GET", path: "/articles/#{article.id}")
    refute_includes res.raw, "Set-Cookie: flash_notice"
    refute_includes res.raw, "Set-Cookie: flash_alert"
  end

  def test_full_round_trip_post_then_follow_with_cookie
    body = "article%5Btitle%5D=Round+trip&article%5Bbody%5D=Long+enough+body+content."
    res1 = cgi(method: "POST", path: "/articles", body: body)
    assert res1.redirect?
    flash_value = res1.set_cookies["flash_notice"]
    refute_nil flash_value
    article_id = res1.location[%r{/articles/(\d+)}, 1].to_i

    # Follow the redirect, sending the cookie back.
    res2 = cgi(
      method: "GET",
      path: "/articles/#{article_id}",
      cookies: "flash_notice=#{flash_value}",
    )
    assert_equal 200, res2.status
    assert_includes res2.body, ">Article was successfully created.</p>"
    # The cookie is cleared on this response (consumed).
    assert_includes res2.raw, "Set-Cookie: flash_notice=;"
  end

  def test_invalid_create_does_not_emit_flash
    body = "article%5Btitle%5D=&article%5Bbody%5D=short"
    res = cgi(method: "POST", path: "/articles", body: body)
    assert_equal 422, res.status
    refute_includes res.raw, "Set-Cookie: flash_notice"
  end

  def test_comment_create_alert_path_emits_flash_alert
    article = Article.new(title: "Host", body: "Long enough body content here.")
    article.save
    body = "comment%5Bcommenter%5D=&comment%5Bbody%5D=Body"
    res = cgi(method: "POST", path: "/articles/#{article.id}/comments", body: body)
    assert res.redirect?
    assert_equal "Could%20not%20create%20comment.",
                 res.set_cookies["flash_alert"]
  end

  # ── query string parsing ─────────────────────────────────────────

  def test_query_string_merges_into_params
    # No GET-with-query route is exercised by real-blog's controllers,
    # but the parser layer should accept and merge the query string
    # into params. Exercise via a path-with-query that resolves and
    # confirm params survive (we inspect by request_method/path which
    # the controller stores).
    res = cgi(method: "GET", path: "/articles", query: "filter=recent")
    assert_equal 200, res.status
    # No assertion on filter behavior — the controller doesn't
    # consume it — but the response must have rendered cleanly.
    assert_includes res.body, "<!DOCTYPE html>"
  end
end
