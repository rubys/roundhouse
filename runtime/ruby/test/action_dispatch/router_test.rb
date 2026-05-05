require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_dispatch/router.rb`.
# Promoted from fixtures/spinel-blog/test/runtime/router_test.rb,
# extended with tests for the index-loop shape (Router.match was
# rewritten from `table.each do |route| ... return ... end` to a
# while loop so JS forEach + early-return survives transpile —
# see commit on 2026-05-04 in runtime/ruby/action_dispatch/router.rb).
# A regression test against that shape would have caught the
# transpile bug before it shipped.
class RouterTest < Minitest::Test
  TABLE = [
    { method: "GET",    pattern: "/articles",     controller: :articles_controller, action: :index   },
    { method: "GET",    pattern: "/articles/:id", controller: :articles_controller, action: :show    },
    { method: "POST",   pattern: "/articles",     controller: :articles_controller, action: :create  },
    { method: "DELETE", pattern: "/articles/:id", controller: :articles_controller, action: :destroy },
    { method: "POST",   pattern: "/articles/:article_id/comments", controller: :comments_controller, action: :create },
    { method: "DELETE", pattern: "/articles/:article_id/comments/:id", controller: :comments_controller, action: :destroy },
  ].freeze

  def test_matches_collection_get
    m = Router.match("GET", "/articles", TABLE)
    refute_nil m
    assert_equal :index, m[:action]
    # path_params is now an HWIA (String-keyed); empty for static
    # match. Compare via length rather than literal Hash equality.
    assert_equal 0, m[:path_params].length
  end

  def test_matches_member_get_and_captures_id
    m = Router.match("GET", "/articles/42", TABLE)
    refute_nil m
    assert_equal :show, m[:action]
    assert_equal "42", m[:path_params][:id]
  end

  def test_method_must_match
    assert_nil Router.match("PUT", "/articles", TABLE)
  end

  def test_returns_nil_when_path_does_not_match
    assert_nil Router.match("GET", "/articles/42/edit", TABLE)
    assert_nil Router.match("GET", "/foo", TABLE)
  end

  def test_captures_nested_resource_params
    m = Router.match("POST", "/articles/7/comments", TABLE)
    refute_nil m
    assert_equal :create, m[:action]
    assert_equal "7", m[:path_params][:article_id]
  end

  def test_captures_doubly_nested_resource_params
    # Regression case: pre-rewrite, Router.match's body was
    # `table.each do |route| ... return ... end`. The TS emitter
    # lowered `each` to `forEach` whose callback's `return`
    # doesn't exit the surrounding function — every match
    # silently dropped. Rewriting to a while-loop with a single
    # `return` from the method body fixed it. This test (which
    # finds a route, returning a non-nil match) would have
    # caught the regression at the framework level.
    m = Router.match("DELETE", "/articles/7/comments/3", TABLE)
    refute_nil m
    assert_equal :destroy, m[:action]
    assert_equal "7", m[:path_params][:article_id]
    assert_equal "3", m[:path_params][:id]
  end

  def test_method_is_case_insensitive
    m = Router.match("get", "/articles", TABLE)
    refute_nil m
    assert_equal :index, m[:action]
  end

  def test_first_match_wins_when_multiple_routes_could_match
    # Two routes can match `/articles` (the literal collection
    # form for index AND a hypothetical member-:id where :id ==
    # "articles"). The literal earlier in the table wins; the
    # iteration must return on first match without continuing.
    table = [
      { method: "GET", pattern: "/articles",     controller: :a, action: :first  },
      { method: "GET", pattern: "/:wildcard",    controller: :a, action: :second },
    ]
    m = Router.match("GET", "/articles", table)
    assert_equal :first, m[:action]
  end

  # ── match_pattern ──
  # Lower-level helper called by match. Tested for parity with
  # the public surface so changes to one half can't drift from
  # the other.

  def test_match_pattern_returns_empty_hash_for_pure_static_match
    assert_equal 0, Router.match_pattern("/articles", "/articles").length
  end

  def test_match_pattern_returns_nil_on_length_mismatch
    assert_nil Router.match_pattern("/articles", "/articles/42")
    assert_nil Router.match_pattern("/articles/:id", "/articles")
  end

  def test_match_pattern_returns_nil_on_literal_segment_mismatch
    assert_nil Router.match_pattern("/articles/:id", "/posts/42")
  end

  def test_match_pattern_captures_one_param
    h = Router.match_pattern("/articles/:id", "/articles/42")
    # HWIA accepts either Symbol or String access; assert via Symbol
    # form so the indifferent surface is exercised.
    assert_equal "42", h[:id]
    assert_equal 1, h.length
  end

  def test_match_pattern_captures_multiple_params
    h = Router.match_pattern("/articles/:article_id/comments/:id", "/articles/7/comments/3")
    assert_equal "7", h[:article_id]
    assert_equal "3", h[:id]
    assert_equal 2, h.length
  end
end
