require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_controller/cookies.rb`.
#
# RUBY-FAMILY ONLY, deliberately. CookieJar lives in a reopen outside
# the strict-target runtime tables (see the header of cookies.rb): a
# CookieJar-typed field on Base must not transpile to crystal/kotlin/
# swift/typescript, which never exercise cookies. `base_test.rb` beside
# this file IS wired into those lanes, so cookie coverage cannot live
# there — hence a separate file, referenced only by
# `framework_tests_ruby` and `framework_tests_spinel`. The bare-source
# `framework_ruby_tests_pass` gate globs `test/**/*_test.rb` and picks
# it up automatically.
class ActionControllerCookiesTest < Minitest::Test
  def setup
    @jar = ActionController::CookieJar.new({"a" => "1", "b" => "2"})
  end

  # ── merged view ─────────────────────────────────────────────
  # `[]` reads inbound-overlaid-by-writes; `to_h` is that same view
  # materialized, and `pending` is the writes-only subset the
  # dispatcher serializes as Set-Cookie. Keeping both named is the
  # point: conflating them would either resend every inbound cookie
  # or hide writes from iteration.

  def test_to_h_merges_inbound_with_pending_writes
    @jar["c"] = "3"
    @jar["a"] = "9"
    assert_equal({"a" => "9", "b" => "2", "c" => "3"}, @jar.to_h)
  end

  def test_pending_is_writes_only_not_the_whole_jar
    @jar["c"] = "3"
    assert_equal({"c" => "3"}, @jar.pending)
  end

  # ── iteration ───────────────────────────────────────────────

  def test_each_yields_every_inbound_pair
    seen = []
    @jar.each { |k, v| seen << "#{k}=#{v}" }
    assert_equal ["a=1", "b=2"], seen
  end

  def test_each_reflects_writes_overlaid_on_inbound
    @jar["c"] = "3"
    @jar["a"] = "9"
    seen = []
    @jar.each { |k, v| seen << "#{k}=#{v}" }
    assert_equal ["a=9", "b=2", "c=3"], seen
  end

  def test_each_normalizes_symbol_keys_to_strings
    jar = ActionController::CookieJar.new({})
    jar[:tag_filters] = "meta"
    seen = []
    jar.each { |k, v| seen << "#{k}=#{v}" }
    assert_equal ["tag_filters=meta"], seen
  end

  # ── the shape this exists for ───────────────────────────────
  # lobsters' `ApplicationController#remove_unknown_cookies` walks the
  # jar and calls `cookies.delete(key)` from inside the block. `delete`
  # records a write into @out, so `each` must walk a snapshot — this is
  # the regression test for that, not an incidental case.

  def test_delete_from_inside_each_is_safe
    walked = []
    @jar.each do |key, _value|
      walked << key
      @jar.delete(key) if key == "b"
    end
    assert_equal ["a", "b"], walked
    assert_equal "1", @jar["a"]
    assert_equal "", @jar["b"]
  end

  def test_delete_from_inside_each_records_the_cleared_write
    @jar.each { |key, _value| @jar.delete(key) if key == "b" }
    assert_equal({"b" => ""}, @jar.pending)
  end
end
