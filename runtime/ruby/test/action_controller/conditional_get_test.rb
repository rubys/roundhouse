# frozen_string_literal: true
require_relative "../test_helper"

# `commit_conditional_get!` (action_controller/conditional_get.rb):
# Rack::ETag, Rack::ConditionalGet and the response's own default
# Cache-Control, over the controller's buffered response.
#
# Every expected header is what a Rails 8.1.4 app (load_defaults 8.1,
# rack 3.2.7) answered for the same action through
# `Rack::MockRequest`: the full default middleware stack, not the
# middleware classes alone. `<p>hi</p>` hashes to the ETag below there.
class ConditionalGetTest < Minitest::Test
  class TestController < ActionController::Base
    def process_action(action_name)
    end
  end

  PAGE_ETAG = "W/\"0a4735281db700223af63abc387c351f\""
  DEFAULT = "max-age=0, private, must-revalidate"

  def page(status: :ok)
    c = TestController.new
    c.render("<p>hi</p>", status: status)
    c
  end

  def test_a_rendered_page_gets_the_body_etag_and_the_default_cache_control
    c = page
    c.commit_conditional_get!("GET", "")
    assert_equal 200, c.status
    assert_equal PAGE_ETAG, c.headers["ETag"]
    assert_equal DEFAULT, c.headers["Cache-Control"]
  end

  def test_a_created_response_is_digested_too
    c = page(status: :created)
    c.commit_conditional_get!("POST", "")
    assert_equal PAGE_ETAG, c.headers["ETag"]
    assert_equal DEFAULT, c.headers["Cache-Control"]
  end

  def test_another_status_gets_no_etag_and_no_cache
    c = page(status: :unprocessable_entity)
    c.commit_conditional_get!("GET", "")
    assert_nil c.headers["ETag"]
    assert_equal "no-cache", c.headers["Cache-Control"]
  end

  def test_a_redirect_gets_no_cache
    c = TestController.new
    c.redirect_to("/page")
    c.commit_conditional_get!("GET", "")
    assert_nil c.headers["ETag"]
    assert_equal "no-cache", c.headers["Cache-Control"]
  end

  def test_an_empty_body_gets_no_cache
    c = TestController.new
    c.render("")
    c.commit_conditional_get!("GET", "")
    assert_nil c.headers["ETag"]
    assert_equal "no-cache", c.headers["Cache-Control"]
  end

  def test_a_stated_cache_control_is_kept
    c = TestController.new
    c.expires_in(60, public: true)
    c.render("cc")
    c.commit_cache_control!
    c.commit_conditional_get!("GET", "")
    assert_equal "max-age=60, public", c.headers["Cache-Control"]
    assert_equal "W/\"355b1bbfc96725cdce8f4a2708fda310\"", c.headers["ETag"]

    c = TestController.new
    c.headers["Cache-Control"] = "no-store"
    c.render("ns")
    c.commit_cache_control!
    c.commit_conditional_get!("GET", "")
    assert_equal "no-store", c.headers["Cache-Control"]
    assert_equal "W/\"7bfba6e0021f1fd1183dfefe60216f13\"", c.headers["ETag"]
  end

  # The action's own validator: no body digest, and the response's
  # default Cache-Control rather than Rack::ETag's no-cache.
  def test_an_action_etag_is_kept_and_gets_the_default_cache_control
    c = TestController.new
    c.headers["ETag"] = "W/\"mine\""
    c.render("me")
    c.commit_conditional_get!("GET", "")
    assert_equal "W/\"mine\"", c.headers["ETag"]
    assert_equal DEFAULT, c.headers["Cache-Control"]
  end

  def test_last_modified_skips_the_digest_and_gets_the_default_cache_control
    c = TestController.new
    c.headers["Last-Modified"] = "Tue, 13 Oct 2026 10:00:00 GMT"
    c.render("lm")
    c.commit_conditional_get!("GET", "")
    assert_nil c.headers["ETag"]
    assert_equal DEFAULT, c.headers["Cache-Control"]
  end

  def test_fresh_when_gets_the_default_cache_control
    c = TestController.new
    # The tag itself is fresh_when's (base.rb); this pins the header beside it.
    c.fresh_when(etag: "abc")
    c.render("st")
    c.commit_conditional_get!("GET", "")
    assert_equal DEFAULT, c.headers["Cache-Control"]
  end

  def test_a_matching_if_none_match_answers_304_without_a_body
    %w[GET HEAD].each do |verb|
      c = page
      c.commit_conditional_get!(verb, PAGE_ETAG)
      assert_equal 304, c.status, verb
      assert_equal "", c.body
      assert_equal "", c.content_type
      assert_equal PAGE_ETAG, c.headers["ETag"]
      assert_equal DEFAULT, c.headers["Cache-Control"]
    end
  end

  # Rack::ConditionalGet compares the two strings: no W/ stripping, no
  # list parsing, GET and HEAD only, 200 only.
  def test_only_an_exact_match_on_a_get_200_answers_304
    [
      ["GET", PAGE_ETAG.sub("W/", ""), :ok],
      ["GET", PAGE_ETAG + ", \"zz\"", :ok],
      ["POST", PAGE_ETAG, :ok],
      ["GET", PAGE_ETAG, :created],
    ].each do |verb, inm, status|
      c = page(status: status)
      c.commit_conditional_get!(verb, inm)
      refute_equal 304, c.status, [verb, inm, status].inspect
      assert_equal "<p>hi</p>", c.body
    end
  end

  def test_an_action_etag_answers_304_when_it_matches
    c = TestController.new
    c.headers["ETag"] = "W/\"mine\""
    c.render("me")
    c.commit_conditional_get!("GET", "W/\"mine\"")
    assert_equal 304, c.status
    assert_equal "", c.body
  end
end
