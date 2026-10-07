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

  # `match "lookup/:id", to: …, via: :all` lowers to one "ANY" row,
  # which Rails answers for every request method.
  def test_any_route_matches_every_method
    table = [ActionDispatch::Router::Route.new("ANY", "/lookup/:id", :widgets_controller, :show)]
    m = ActionDispatch::Router.match("DELETE", "/lookup/7", table)
    raise "expected match" if m.nil?
    assert_equal :show, m.action
    m = ActionDispatch::Router.match("GET", "/lookup/7", table)
    raise "expected match" if m.nil?
    assert_equal "7", m.path_params["id"]
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

  # A capture follows path escaping, so literal plus is not a form-space.
  def test_path_captures_decode_percent_escapes_once_and_preserve_plus
    h = ActionDispatch::Router.match_pattern("/echo/:value", "/echo/+%2B%20%252F")
    raise "expected match" if h.nil?
    assert_equal "++ %2F", h["value"]
  end

  # Literal UTF-8 and escaped multibyte characters must share the same output.
  def test_path_captures_decode_utf8_and_preserve_literal_unicode
    h = ActionDispatch::Router.match_pattern("/echo/:value", "/echo/café%20%E6%9D%B1%E4%BA%AC%F0%9F%8E%89")
    raise "expected match" if h.nil?
    assert_equal "café 東京🎉", h["value"]
  end

  # Percent-encoded invalid sequences are portable to every target String.
  # Raw binary transport is exercised separately by Ruby/Spinel/Elixir probes.
  def test_invalid_utf8_is_rejected_after_byte_decoding
    ["%FF", "%E0%80%AF", "%ED%A0%80", "%F4%90%80%80", "%C2"].each do |segment|
      assert_raises(ArgumentError) do
        ActionDispatch::Router.match_pattern("/echo/:value", "/echo/" + segment)
      end
    end
    h = ActionDispatch::Router.match_pattern("/echo/:value", "/echo/%25FF")
    raise "expected match" if h.nil?
    assert_equal "%FF", h["value"]
  end

  # Bounds rejection cannot mistake the valid zero byte for a missing value.
  def test_checked_byte_access_preserves_zero_and_rejects_missing_offsets
    zero_byte = ActionDispatch::Router.capture_byte([0, 255], 0)
    maximum_byte = ActionDispatch::Router.capture_byte([0, 255], 1)
    assert_equal 0, zero_byte
    assert_equal 255, maximum_byte
    assert_raises(ArgumentError) { ActionDispatch::Router.capture_byte([0], -1) }
    assert_raises(ArgumentError) { ActionDispatch::Router.capture_byte([0], 1) }
  end

  # NUL is a real capture byte, not a terminator or a reason to decode twice.
  def test_nul_capture_preserves_bytes_and_decodes_once
    h = ActionDispatch::Router.match_pattern("/echo/:value", "/echo/%00")
    raise "expected match" if h.nil?
    assert_equal "\0", h["value"]
    h = ActionDispatch::Router.match_pattern("/echo/:value", "/echo/%2500")
    raise "expected match" if h.nil?
    assert_equal "%00", h["value"]
  end

  # Escaped separators belong to a matched capture, including glob captures.
  def test_prefixed_and_glob_captures_decode_after_segmentation
    h = ActionDispatch::Router.match_pattern("/~:name", "/~alice%2Bbob")
    raise "expected match" if h.nil?
    assert_equal "alice+bob", h["name"]
    h = ActionDispatch::Router.match_pattern("/files/*name", "/files/dir%2finside/file%20name")
    raise "expected match" if h.nil?
    assert_equal "dir/inside/file name", h["name"]
  end

  # An escaped dot cannot retrospectively change the route format suffix.
  def test_encoded_dot_is_part_of_the_capture_not_a_format
    table = [ActionDispatch::Router::Route.new("GET", "/echo/:value", :echo_controller, :show)]
    m = ActionDispatch::Router.match("GET", "/echo/1%2Ejson", table)
    raise "expected match" if m.nil?
    assert_equal "1.json", m.path_params["value"]
    assert_equal false, m.path_params.key?("format")
  end

  # Decoding cannot turn a different static path or failed constraint into a match.
  def test_static_segments_and_constraints_match_before_decoding
    assert_nil ActionDispatch::Router.match_pattern("/echo/:value", "/%65cho/1")
    assert_nil ActionDispatch::Router.match_pattern("/echo/:value", "/echo/%31", "value")
  end

  # A rejected route must not interpret bytes that belong to another candidate.
  def test_invalid_encoding_in_a_nonmatching_pattern_does_not_raise
    assert_nil ActionDispatch::Router.match_pattern("/echo/:value/edit", "/echo/%FF/other")
  end

  # A later invalid capture cannot expose a partially decoded match. A new
  # request must still decode both captures and the independent format key.
  def test_multiple_captures_remain_atomic_after_invalid_encoding
    assert_raises(ArgumentError) do
      ActionDispatch::Router.match_pattern("/pair/:first/:second", "/pair/good%2B/%FF")
    end
    table = [ActionDispatch::Router::Route.new("GET", "/pair/:first/:second", :echo_controller, :show)]
    m = ActionDispatch::Router.match("GET", "/pair/one%2B/two%20words.json", table)
    raise "expected match" if m.nil?
    assert_equal "one+", m.path_params["first"]
    assert_equal "two words", m.path_params["second"]
    assert_equal "json", m.path_params["format"]
  end

  # Incomplete or nonhex escapes remain literal path bytes, as in Rails.
  def test_malformed_percent_syntax_is_retained
    h = ActionDispatch::Router.match_pattern("/echo/:value", "/echo/%GG%2%")
    raise "expected match" if h.nil?
    assert_equal "%GG%2%", h["value"]
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

  # Lobsters' `/~:username` — a literal prefix before the param, inside
  # one segment. Rails binds the rest of the segment.
  def test_match_pattern_captures_a_prefixed_param
    h = ActionDispatch::Router.match_pattern("/~:username/threads", "/~alice/threads")
    raise "expected match" if h.nil?
    assert_equal "alice", h["username"]
    assert_equal 1, h.length
  end

  def test_match_pattern_rejects_a_missing_prefix_or_empty_value
    assert_nil ActionDispatch::Router.match_pattern("/~:username", "/alice")
    assert_nil ActionDispatch::Router.match_pattern("/~:username", "/~")
    assert_nil ActionDispatch::Router.match_pattern("/~:username", "/@alice")
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

  # Rails compiles a dynamic segment to `[^/.?]+`, so `(.:format)` is
  # peeled off BEFORE any segment binds. Matching the literal path first
  # instead let `:id` swallow the extension — `/articles/42.json` bound
  # `id = "42.json"` with no format, and since `find` coerces that back
  # to 42 the action ran and only the RESPONSE FORMAT was lost. The
  # collection form (`/articles.json`) never had the bug: its last
  # segment is a literal that cannot swallow anything, which is exactly
  # what hid it.
  def test_member_path_format_extension_is_stripped_not_captured
    m = ActionDispatch::Router.match("GET", "/articles/42.json", TABLE)
    raise "expected match" if m.nil?
    assert_equal :show, m.action
    assert_equal "42", m.path_params["id"]
    assert_equal "json", m.path_params["format"]
  end

  def test_collection_path_format_extension_is_stripped
    m = ActionDispatch::Router.match("GET", "/articles.json", TABLE)
    raise "expected match" if m.nil?
    assert_equal :index, m.action
    assert_equal "json", m.path_params["format"]
  end

  # An extension that names no registered format is not one: a dotted
  # segment stays whole, which is what keeps `/domains/example.com`
  # routing to `:id = "example.com"` — Rails' own answer for that URL.
  def test_unregistered_extension_stays_in_the_segment
    m = ActionDispatch::Router.match("GET", "/articles/example.com", TABLE)
    raise "expected match" if m.nil?
    assert_equal "example.com", m.path_params["id"]
    # `fetch(k, default)` rather than `["format"]`: a missing Hash key
    # raises on Crystal, so an ABSENCE assertion has to read the key the
    # way both dispatchers read it.
    assert_equal "", m.path_params.fetch("format", "")
  end

  # The literal path is the FALLBACK, so a route whose path genuinely
  # ends in a format-shaped extension still wins as itself.
  def test_literal_dotted_route_wins_when_the_stripped_form_matches_nothing
    table = [
      ActionDispatch::Router::Route.new("GET", "/manifest.json", :pwa_controller, :manifest),
    ]
    m = ActionDispatch::Router.match("GET", "/manifest.json", table)
    raise "expected match" if m.nil?
    assert_equal :manifest, m.action
    assert_equal "", m.path_params.fetch("format", "")
  end

  # `*filename` — the glob Rails puts last on the Active Storage
  # engine's routes. It takes every remaining segment, slash-joined,
  # and the `.ext` peel still applies to the last one (Rails' `format:
  # false` on those routes is not modeled; the segment is cosmetic).
  def test_glob_segment_takes_the_rest_of_the_path
    table = [
      ActionDispatch::Router::Route.new(
        "GET", "/rails/active_storage/disk/:encoded_key/*filename", :active_storage_disk, :show
      ),
    ]
    m = ActionDispatch::Router.match("GET", "/rails/active_storage/disk/abc/dir/moon.jpg", table)
    raise "expected match" if m.nil?
    assert_equal "abc", m.path_params["encoded_key"]
    assert_equal "dir/moon", m.path_params["filename"]
    assert_equal "jpg", m.path_params["format"]
    assert_nil ActionDispatch::Router.match("GET", "/rails/active_storage/disk/abc", table)
  end
  # Direct decode_capture checks (match goes through decode_captures).
  # UTF-8 sequences decode; malformed `%` syntax stays literal.
  def test_a_capture_is_percent_decoded
    assert_equal "a b+c", ActionDispatch::Router.decode_capture("a%20b+c")
    assert_equal "a/b?#", ActionDispatch::Router.decode_capture("a%2fb%3F%23")
    assert_equal "100%", ActionDispatch::Router.decode_capture("100%25")
    assert_equal "bad%zz%2", ActionDispatch::Router.decode_capture("bad%zz%2")
    assert_equal "josé", ActionDispatch::Router.decode_capture("jos%C3%A9")
    assert_equal "plain", ActionDispatch::Router.decode_capture("plain")
  end

  # The inverse a routing redirect's `%{name}` needs: Rails' PATH set
  # escaped over the decoded capture (including UTF-8 bytes as `%XX`).
  def test_escape_path_reverses_a_decoded_capture
    assert_equal "a%23top%3Fx%20y/b:c@d!e%5Bf%5D", ActionDispatch::Router.escape_path("a#top?x y/b:c@d!e[f]")
    assert_equal "100%25", ActionDispatch::Router.escape_path("100%")
    assert_equal "jos%C3%A9", ActionDispatch::Router.escape_path("josé")
    assert_equal "x%0Ay", ActionDispatch::Router.escape_path("x\ny")
    assert_equal "a/b~", ActionDispatch::Router.escape_path("a/b~")
  end
end
