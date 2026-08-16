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

  # ── values are Strings on the wire ──────────────────────────
  # A cookie has no type; Rails serializes whatever it is given. The
  # store is declared Hash[String, String] and every reader treats it
  # that way, so the coercion belongs at the write, not at each read.
  # campfire's welcome_controller_test seeds `cookies[:last_room] =
  # room.id` — an Integer.

  def test_a_non_string_value_is_stored_as_a_string
    @jar[:last_room] = 7
    assert_equal "7", @jar["last_room"]
    assert_equal({"last_room" => "7"}, @jar.pending)
  end

  # ── signing round-trip ──────────────────────────────────────
  # The `signed` view is the half campfire's whole session depends on,
  # and nothing covered it. A verified read must give back exactly what
  # was written; anything that does not verify must be indistinguishable
  # from absent.

  def test_signed_write_then_read_round_trips
    jar = ActionController::CookieJar.new({})
    jar.signed[:session_token] = "abc123"
    refute_equal "abc123", jar["session_token"], "the stored cookie must be signed, not plaintext"
    assert_equal "abc123", jar.signed[:session_token]
  end

  def test_signed_accepts_the_options_hash_write_form
    # `cookies.signed.permanent[:k] = {value:, httponly:, same_site:}`
    # — how campfire's Authentication concern writes the session.
    jar = ActionController::CookieJar.new({})
    jar.signed.permanent[:session_token] = { value: "tok", httponly: true, same_site: :lax }
    assert_equal "tok", jar.signed[:session_token]
  end

  def test_a_tampered_signed_cookie_reads_as_absent
    jar = ActionController::CookieJar.new({})
    jar.signed[:session_token] = "abc123"
    tampered = ActionController::CookieJar.new({"session_token" => jar["session_token"] + "x"})
    assert_equal "", tampered.signed[:session_token]
  end

  def test_a_cookie_signed_for_another_name_does_not_verify
    jar = ActionController::CookieJar.new({})
    jar.signed[:session_token] = "abc123"
    moved = ActionController::CookieJar.new({"other_token" => jar["session_token"]})
    assert_equal "", moved.signed[:other_token]
  end

  # ── Rails' own spelling ─────────────────────────────────────

  def test_to_hash_is_the_merged_view
    @jar["c"] = "3"
    assert_equal @jar.to_h, @jar.to_hash
  end

  def test_action_dispatch_cookie_jar_builds_onto_the_same_implementation
    jar = ActionDispatch::Cookies::CookieJar.build(nil, {"a" => "1"})
    assert_instance_of ActionController::CookieJar, jar
    assert_equal "1", jar["a"]
  end
end
