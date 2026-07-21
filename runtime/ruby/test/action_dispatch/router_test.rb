require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_dispatch/router.rb`.
# Promoted from fixtures/spinel-blog/test/runtime/router_test.rb,
# extended with tests for the index-loop shape (ActionDispatch::Router.match was
# rewritten from `table.each do |route| ... return ... end` to a
# while loop so JS forEach + early-return survives transpile —
# see commit on 2026-05-04 in runtime/ruby/action_dispatch/router.rb).
# A regression test against that shape would have caught the
# transpile bug before it shipped.
class RouterTest < Minitest::Test
  # Bring `ActionDispatch::Router` into scope as `Router` for test
  # readability — the source declares the canonical Rails-style nested
  # path; bare refs from app code follow Ruby's `include` convention.
  include ActionDispatch

  # Route rows are typed `ActionDispatch::Router::Route` instances now;
  # the prior `Hash[Symbol, untyped]` shape no longer round-trips
  # through strict-typed targets. Positional constructor matches the
  # `def initialize(verb, pattern, controller, action)` signature.
  TABLE = [
    ActionDispatch::Router::Route.new("GET",    "/articles",     :articles_controller, :index),
    ActionDispatch::Router::Route.new("GET",    "/articles/:id", :articles_controller, :show),
    ActionDispatch::Router::Route.new("POST",   "/articles",     :articles_controller, :create),
    ActionDispatch::Router::Route.new("DELETE", "/articles/:id", :articles_controller, :destroy),
    ActionDispatch::Router::Route.new("POST",   "/articles/:article_id/comments", :comments_controller, :create),
    ActionDispatch::Router::Route.new("DELETE", "/articles/:article_id/comments/:id", :comments_controller, :destroy),
  ].freeze

  # Raise-if-nil instead of `refute_nil` because Crystal's flow
  # analysis narrows `m` to non-nil after a raise-on-nil but not
  # after a `refute_nil` call (the assertion is opaque to the
  # compiler). CRuby behavior unchanged — both forms abort the
  # test on a nil match.
  def test_matches_collection_get
    # Static-pattern collection match: the path_params is empty for
    # this case; the per-key assertions live on member-shape tests
    # below (`/articles/:id` etc.). Avoids depending on the body-
    # typer's chain-return propagation through MatchResult.path_params
    # which doesn't yet reach the `.length`/`[]` Hash rewrites on every
    # target.
    m = ActionDispatch::Router.match("GET", "/articles", TABLE)
    raise "expected match" if m.nil?
    assert_equal :index, m.action
  end

  def test_matches_member_get_and_captures_id
    m = ActionDispatch::Router.match("GET", "/articles/42", TABLE)
    raise "expected match" if m.nil?
    assert_equal :show, m.action
    assert_equal "42", m.path_params["id"]
  end

  def test_method_must_match
    assert_nil ActionDispatch::Router.match("PUT", "/articles", TABLE)
  end

  def test_returns_nil_when_path_does_not_match
    assert_nil ActionDispatch::Router.match("GET", "/articles/42/edit", TABLE)
    assert_nil ActionDispatch::Router.match("GET", "/foo", TABLE)
  end

  def test_captures_nested_resource_params
    m = ActionDispatch::Router.match("POST", "/articles/7/comments", TABLE)
    raise "expected match" if m.nil?
    assert_equal :create, m.action
    assert_equal "7", m.path_params["article_id"]
  end

  def test_captures_doubly_nested_resource_params
    # Regression case: pre-rewrite, ActionDispatch::Router.match's body was
    # `table.each do |route| ... return ... end`. The TS emitter
    # lowered `each` to `forEach` whose callback's `return`
    # doesn't exit the surrounding function — every match
    # silently dropped. Rewriting to a while-loop with a single
    # `return` from the method body fixed it. This test (which
    # finds a route, returning a non-nil match) would have
    # caught the regression at the framework level.
    m = ActionDispatch::Router.match("DELETE", "/articles/7/comments/3", TABLE)
    raise "expected match" if m.nil?
    assert_equal :destroy, m.action
    assert_equal "7", m.path_params["article_id"]
    assert_equal "3", m.path_params["id"]
  end

  def test_method_is_case_insensitive
    m = ActionDispatch::Router.match("get", "/articles", TABLE)
    raise "expected match" if m.nil?
    assert_equal :index, m.action
  end

  def test_first_match_wins_when_multiple_routes_could_match
    # Two routes can match `/articles` (the literal collection
    # form for index AND a hypothetical member-:id where :id ==
    # "articles"). The literal earlier in the table wins; the
    # iteration must return on first match without continuing.
    table = [
      ActionDispatch::Router::Route.new("GET", "/articles",  :a, :first),
      ActionDispatch::Router::Route.new("GET", "/:wildcard", :a, :second),
    ]
    m = ActionDispatch::Router.match("GET", "/articles", table)
    raise "expected match" if m.nil?
    assert_equal :first, m.action
  end

  # ── match_pattern ──
  # Lower-level helper called by match. Tested for parity with
  # the public surface so changes to one half can't drift from
  # the other.

  def test_match_pattern_returns_empty_hash_for_pure_static_match
    h = ActionDispatch::Router.match_pattern("/articles", "/articles")
    raise "expected match" if h.nil?
    assert_equal 0, h.length
  end

  def test_match_pattern_returns_nil_on_length_mismatch
    assert_nil ActionDispatch::Router.match_pattern("/articles", "/articles/42")
    assert_nil ActionDispatch::Router.match_pattern("/articles/:id", "/articles")
  end

  def test_match_pattern_returns_nil_on_literal_segment_mismatch
    assert_nil ActionDispatch::Router.match_pattern("/articles/:id", "/posts/42")
  end

  def test_match_pattern_captures_one_param
    h = ActionDispatch::Router.match_pattern("/articles/:id", "/articles/42")
    raise "expected match" if h.nil?
    assert_equal "42", h["id"]
    assert_equal 1, h.length
  end

  def test_match_pattern_captures_multiple_params
    h = ActionDispatch::Router.match_pattern("/articles/:article_id/comments/:id", "/articles/7/comments/3")
    raise "expected match" if h.nil?
    assert_equal "7", h["article_id"]
    assert_equal "3", h["id"]
    assert_equal 2, h.length
  end

  # ── int_params (digit-only constraints) ──
  # Roda's `Integer` matcher and Rails digit-class `constraints:`
  # lower to `Route.new(..., nil, "id")` (space-joined constraint
  # list). A constrained segment that isn't all digits makes the route
  # a non-match — without this, `/articles/12abc` would bind
  # `id = "12abc"` and (post `to_i`) serve article 12 where the source
  # app 404s.

  INT_TABLE = [
    ActionDispatch::Router::Route.new("GET", "/articles/:id", :articles_controller, :show, nil, "id"),
  ].freeze

  def test_int_param_matches_digits
    m = ActionDispatch::Router.match("GET", "/articles/42", INT_TABLE)
    raise "expected match" if m.nil?
    assert_equal :show, m.action
    assert_equal "42", m.path_params["id"]
  end

  def test_int_param_rejects_digit_prefixed_garbage
    assert_nil ActionDispatch::Router.match("GET", "/articles/12abc", INT_TABLE)
  end

  def test_int_param_rejects_non_digits
    assert_nil ActionDispatch::Router.match("GET", "/articles/abc", INT_TABLE)
  end

  def test_int_param_accepts_leading_zeros
    # Roda's `Integer` matcher accepts "007" (it's id 7) — a `to_i`
    # round-trip check would wrongly 404 it.
    m = ActionDispatch::Router.match("GET", "/articles/007", INT_TABLE)
    raise "expected match" if m.nil?
    assert_equal "007", m.path_params["id"]
  end

  def test_rejected_int_param_falls_through_to_later_route
    table = [
      ActionDispatch::Router::Route.new("GET", "/:id", :a, :constrained, nil, "id"),
      ActionDispatch::Router::Route.new("GET", "/:slug", :a, :fallback),
    ]
    m = ActionDispatch::Router.match("GET", "/about", table)
    raise "expected match" if m.nil?
    assert_equal :fallback, m.action
  end

  def test_unconstrained_route_still_captures_arbitrary_segments
    m = ActionDispatch::Router.match("GET", "/articles/12abc", TABLE)
    raise "expected match" if m.nil?
    assert_equal "12abc", m.path_params["id"]
  end
end
