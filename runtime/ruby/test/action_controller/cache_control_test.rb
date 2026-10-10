require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_controller/base.rb`'s
# `ActionController::CacheControlStore` and `Base#expires_in` — the
# PRE-Hash-surface half of rubys/roundhouse#679 (the typed store and
# the strict-target-safe writer). The Hash-like `[]`/`[]=`/`delete`/
# `merge!`/`replace` surface (`cache_control.rb`, ruby-family only)
# gets its own tests appended below once it lands; this file covers
# what is reachable without it.
#
# Every pin here is a `to_header` string, checked against Rails
# 8.1.4's `ActionDispatch::Http::Cache::Response#cache_control_header`
# (verified by running the gem) rather than guessed from the RFC —
# branch selection (no_store / no_cache / otherwise) and field order
# both.
class CacheControlStoreTest < Minitest::Test
  def setup
    @store = ActionController::CacheControlStore.new
  end

  # ── empty ───────────────────────────────────────────────────

  def test_a_fresh_store_is_empty_and_has_no_header
    assert @store.empty?
    assert_equal "", @store.to_header
  end

  # ── no_store branch ─────────────────────────────────────────

  def test_private_and_no_store
    @store.private = true
    @store.no_store = true
    assert_equal "private, no-store", @store.to_header
  end

  # `no_store` wins over `public`/`max_age` even when both are also
  # set — the no_store branch only ever reads `private`/
  # `must_understand` of the flags it does not itself name.
  def test_no_store_wins_over_public_and_max_age
    @store.no_store = true
    @store.public = true
    @store.max_age = 5
    assert_equal "no-store", @store.to_header
    refute @store.empty?
  end

  def test_no_store_with_must_understand
    @store.no_store = true
    @store.must_understand = true
    assert_equal "must-understand, no-store", @store.to_header
  end

  # ── no_cache branch ──────────────────────────────────────────

  def test_no_cache_alone
    @store.no_cache = true
    assert_equal "no-cache", @store.to_header
  end

  def test_no_cache_with_public
    @store.no_cache = true
    @store.public = true
    assert_equal "public, no-cache", @store.to_header
  end

  def test_no_cache_keeps_extra_directives_after_the_cache_flag
    @store.no_cache = true
    @store.extras = ["foo=bar"]
    assert_equal "no-cache, foo=bar", @store.to_header
  end

  # ── otherwise (default) branch ───────────────────────────────

  def test_public_alone
    @store.public = true
    assert_equal "public", @store.to_header
  end

  # The unset-flag default is "private", not merely "not public" —
  # Rails' default branch always emits one or the other.
  def test_neither_public_nor_private_set_renders_private
    @store.max_age = 60
    assert_equal "max-age=60, private", @store.to_header
  end

  def test_must_revalidate
    @store.max_age = 60
    @store.must_revalidate = true
    assert_equal "max-age=60, private, must-revalidate", @store.to_header
  end

  # A clear (what the Hash surface's `delete`/`replace` reduce to)
  # drops `public` back to the "private" default even with other
  # fields still stated.
  def test_clearing_public_after_it_was_set_falls_back_to_private
    @store.max_age = 60
    @store.public = true
    @store.public = false
    assert_equal "max-age=60, private", @store.to_header
  end

  # A STATED zero still renders `max-age=0` — the presence bool, not
  # the Integer's truthiness, gates the segment.
  def test_stated_zero_max_age_still_renders
    @store.max_age = 0
    @store.public = false
    assert_equal "max-age=0, private", @store.to_header
  end

  def test_immutable_and_extras
    @store.max_age = 60
    @store.public = true
    @store.immutable = true
    @store.extras = ["foo=bar"]
    assert_equal "max-age=60, public, immutable, foo=bar", @store.to_header
  end

  # ── clear ────────────────────────────────────────────────────

  def test_clear_resets_every_field
    @store.max_age = 60
    @store.public = true
    @store.no_store = true
    @store.extras = ["x"]
    @store.clear
    assert @store.empty?
    assert_equal "", @store.to_header
  end
end

# `expires_in` — the typed writer every target carries. Exercised
# through a Base subclass rather than the store directly so the
# `cache_control_max_age` / `cache_control_public` delegation (every
# pre-existing caller's surface) is covered alongside the new fields.
class ActionControllerExpiresInTest < Minitest::Test
  class TestController < ActionController::Base
    def process_action(action_name)
    end
  end

  def setup
    @controller = TestController.new
  end

  # The typed store underneath `expires_in`, for the parts of its
  # output `cache_control_max_age` / `cache_control_public` alone
  # cannot show (field order, stale-while-revalidate, stale-if-error,
  # must-revalidate, immutable). `cache_control.rb`'s Hash surface
  # reopens this same object as `response.cache_control`; peeking the
  # ivar directly here is what lets this file cover `expires_in`
  # before that reopen exists.
  def store
    @controller.instance_variable_get(:@cache_control)
  end

  def test_expires_in_public_true
    @controller.expires_in(180, public: true)
    assert_equal "max-age=180, public", store.to_header
    assert_equal 180, @controller.cache_control_max_age
    assert @controller.cache_control_public
  end

  def test_expires_in_defaults_to_private
    @controller.expires_in(180)
    assert_equal "max-age=180, private", store.to_header
    refute @controller.cache_control_public
  end

  def test_expires_in_zero_max_age_explicit_public_false
    @controller.expires_in(0, public: false)
    assert_equal "max-age=0, private", store.to_header
  end

  def test_expires_in_with_stale_while_revalidate_and_stale_if_error
    @controller.expires_in(15, public: true, stale_while_revalidate: 30, stale_if_error: 86_400)
    assert_equal "max-age=15, public, stale-while-revalidate=30, stale-if-error=86400", store.to_header
  end

  def test_expires_in_with_must_revalidate
    @controller.expires_in(60, must_revalidate: true)
    assert_equal "max-age=60, private, must-revalidate", store.to_header
  end

  # `expires_in` deletes `:no_store` — a prior no_store/private state
  # (what the Hash surface's `replace(private: true, no_store: true)`
  # would have left) does not survive a following `expires_in`.
  def test_expires_in_clears_a_prior_no_store
    store.private = true
    store.no_store = true
    @controller.expires_in(60, public: true)
    assert_equal "max-age=60, public", store.to_header
  end

  # An option NOT passed to this call overwrites whatever a prior
  # `expires_in` stated for it — `stale_while_revalidate:` here does
  # not survive a second call that omits it.
  def test_expires_in_without_stale_while_revalidate_clears_a_prior_one
    @controller.expires_in(60, public: true, stale_while_revalidate: 30)
    @controller.expires_in(60, public: true)
    assert_equal "max-age=60, public", store.to_header
  end
end

# `response.cache_control`'s Hash-like surface
# (`action_controller/cache_control.rb`, ruby-family only): `[]`,
# `[]=`, `delete`, `merge!`, `replace`, and `commit_cache_control!`
# (the wire-side write onto the buffered `headers` store). RUBY-FAMILY
# ONLY, like `cookies_test.rb` beside this file — see that file's own
# header comment for why coverage for a reopen outside the strict-
# target tables lives in a file of its own rather than beside
# `ac_base_test.rb`'s.
class CacheControlHashSurfaceTest < Minitest::Test
  class TestController < ActionController::Base
    def process_action(action_name)
    end
  end

  def setup
    @controller = TestController.new
  end

  def cache_control
    @controller.cache_control
  end

  # ── replace / merge! ─────────────────────────────────────────

  def test_replace_is_the_before_action_shape_679_asked_for
    cache_control.replace(private: true, no_store: true)
    assert_equal "private, no-store", cache_control.to_header
  end

  def test_merge_bang_no_store_wins_over_public_and_max_age
    cache_control.merge!(no_store: true, public: true, max_age: 5)
    assert_equal "no-store", cache_control.to_header
  end

  def test_replace_clears_whatever_a_prior_replace_left
    cache_control.replace(private: true, no_store: true)
    cache_control.replace(no_cache: true)
    assert_equal "no-cache", cache_control.to_header
  end

  # ── [] / []= ─────────────────────────────────────────────────

  def test_bracket_assign_public_alone
    cache_control[:public] = true
    assert_equal "public", cache_control.to_header
  end

  def test_bracket_read_is_nil_not_false_when_unset
    assert_nil cache_control[:public]
    cache_control[:public] = true
    assert_equal true, cache_control[:public]
  end

  # A STATED zero `max_age` reads back as `0`, not nil.
  def test_bracket_read_max_age_present_even_when_zero
    cache_control[:max_age] = 0
    assert_equal 0, cache_control[:max_age]
  end

  def test_bracket_assign_coerces_integer_fields_with_to_i
    cache_control[:max_age] = "45"
    assert_equal 45, cache_control[:max_age]
  end

  def test_bracket_assign_nil_clears_a_bool_flag
    cache_control[:public] = true
    cache_control[:public] = nil
    assert_nil cache_control[:public]
  end

  def test_bracket_assign_unrecognized_key_raises
    assert_raises(ArgumentError) { cache_control[:s_maxage] = 60 }
  end

  # ── delete ───────────────────────────────────────────────────

  def test_delete_returns_the_prior_value_and_clears_it
    cache_control[:public] = true
    assert_equal true, cache_control.delete(:public)
    assert_nil cache_control[:public]
  end

  def test_delete_of_an_unset_key_returns_nil
    assert_nil cache_control.delete(:public)
  end

  # ── expires_in interaction (the two order-dependent pins) ────

  def test_expires_in_then_replace_the_replace_wins
    @controller.expires_in(60, public: true)
    cache_control.replace(private: true, no_store: true)
    assert_equal "private, no-store", cache_control.to_header
  end

  def test_replace_then_expires_in_the_expires_in_wins
    cache_control.replace(private: true, no_store: true)
    @controller.expires_in(60, public: true)
    assert_equal "max-age=60, public", cache_control.to_header
  end

  def test_expires_in_then_delete_public_falls_back_to_private
    @controller.expires_in(60, public: true)
    cache_control.delete(:public)
    assert_equal "max-age=60, private", cache_control.to_header
  end

  # ── commit_cache_control! (the wire-side write) ──────────────

  def test_commit_cache_control_writes_the_header_when_the_store_is_not_empty
    cache_control.replace(private: true, no_store: true)
    @controller.commit_cache_control!
    assert_equal "private, no-store", @controller.headers["Cache-Control"]
  end

  def test_commit_cache_control_leaves_a_direct_header_write_alone_when_the_store_is_empty
    @controller.headers["Cache-Control"] = "max-age=0, private"
    @controller.commit_cache_control!
    assert_equal "max-age=0, private", @controller.headers["Cache-Control"]
  end

  # ── merging a directly-written header with the store (#694 review) ──
  # Every expected string below is Rails 8.1.4's own
  # (ActionDispatch::Http::Cache::Response#merge_and_normalize_cache_
  # control!, verified running a real ActionDispatch::Response against
  # the gem) — not guessed from the RFC.
  #
  # The bug: a `before_action`/filter calls `expires_in`, then (or
  # before) the action writes `headers["Cache-Control"]` directly —
  # two independent writes to the same header, same request.
  # `commit_cache_control!` used to replace the header with the
  # store's rendering OUTRIGHT whenever the store was non-empty,
  # discarding anything the existing header carried that the store
  # didn't ALSO state. Rails instead merges: the store's stated
  # directives win over the existing header's, but what the existing
  # header alone carries (an extras token, `private`/`must-understand`
  # under a store that states `no_store` without restating them)
  # survives.

  # The review's own example. Pinned exactly (Rails 8.1.4): the
  # explicit `private, no-store` does NOT survive — `expires_in`
  # states `max_age`/`public`, and Rails' merge deletes any `no-store`
  # the existing header carried before layering the store on (its own
  # comment: "any caching directive coming from a controller overrides
  # no-cache/no-store in the default Cache-Control header"), and the
  # three-branch composer's "otherwise" arm never reads `:private` at
  # all once `:public` is true. So THIS PARTICULAR pin is identical
  # before and after the fix — confirmed against both the gem and the
  # unpatched runtime — because every byte of the existing header here
  # falls in the set Rails discards unconditionally once the store
  # states anything. It stays as a Rails-accuracy regression pin;
  # `test_an_existing_private_and_must_understand_survive_a_bare_
  # no_store_store` below is what actually distinguishes the fixed
  # merge from the old outright-replace.
  def test_expires_in_then_a_direct_header_write_the_store_wins
    @controller.expires_in(60, public: true)
    @controller.headers["Cache-Control"] = "private, no-store"
    @controller.commit_cache_control!
    assert_equal "max-age=60, public", @controller.headers["Cache-Control"]
  end

  # Same note as above: also identical before/after the fix for the
  # same reason (a directly-written `no-cache` is exactly the kind of
  # content Rails' merge discards once the store states anything, and
  # the store alone already renders "max-age=60, public"). Order
  # doesn't matter either way — Rails' own merge is order-independent
  # (both are just mutated state read once at commit time) — checked
  # here with the header written FIRST and `expires_in` called after.
  def test_a_direct_no_cache_header_does_not_survive_a_stated_store
    @controller.headers["Cache-Control"] = "no-cache"
    @controller.expires_in(60, public: true)
    @controller.commit_cache_control!
    assert_equal "max-age=60, public", @controller.headers["Cache-Control"]
  end

  # This one DOES distinguish the fix: the existing header's `private`
  # and `must-understand` are outside the set Rails' merge ever
  # touches (only `no-cache`/`no-store` get deleted before the store
  # is layered on), so they must survive even though the store states
  # only `no_store: true` and nothing else. The unpatched
  # commit_cache_control! replaced the header outright with the
  # store's own to_header ("no-store" alone, dropping both) — this
  # fails without the merge and passes with it.
  def test_an_existing_private_and_must_understand_survive_a_bare_no_store_store
    @controller.headers["Cache-Control"] = "private, must-understand, no-store"
    cache_control.merge!(no_store: true)
    @controller.commit_cache_control!
    assert_equal "private, must-understand, no-store", @controller.headers["Cache-Control"]
  end

  # An existing header's unrecognized directive (not one of Rails'
  # seven special keys) rides through as `:extras`, store's own extras
  # first, deduplicated — same shape `cache_control_headers` gives it.
  # Also distinguishes the fix: the unpatched code dropped both
  # `max-age=10` and the extras token outright.
  def test_an_existing_extras_directive_survives_the_merge
    @controller.headers["Cache-Control"] = "community=UCI, max-age=10"
    cache_control.replace(public: true)
    @controller.commit_cache_control!
    assert_equal "max-age=10, public, community=UCI", @controller.headers["Cache-Control"]
  end
end
