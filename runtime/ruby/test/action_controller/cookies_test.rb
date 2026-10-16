# frozen_string_literal: true
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
    jar.signed.permanent[:session_token] = { value: "tok", httponly: true, same_site: :lax, secure: true }
    assert_equal "tok", jar.signed[:session_token]
    assert jar.flag_httponly?(:session_token)
    assert_equal "Lax", jar.flag_samesite(:session_token)
    assert jar.flag_secure?(:session_token)
  end

  # ── permanence ───────────────────────────────────────────────
  # `permanent` was the identity until 2026-10-06, so every "permanent"
  # cookie went out without Expires and died with the browser session:
  # campfire's sign-in did not survive a restart. Each spelling Rails
  # accepts must record the twenty-year expiry, and nothing else may.

  def test_permanent_writes_carry_a_twenty_year_expiry
    jar = ActionController::CookieJar.new({})
    jar.permanent[:last_room] = 7
    assert_equal "7", jar[:last_room], "a permanent write is still an ordinary write"
    assert_equal "7", jar.permanent[:last_room]
    exp = jar.flag_expires(:last_room)
    assert_match(/\A\w{3}, \d{2} \w{3} \d{4} \d{2}:\d{2}:\d{2} GMT\z/, exp)
    assert_equal (Time.now.utc.year + 20).to_s, exp.split(" ")[3]
  end

  def test_signed_permanent_in_either_order_carries_the_expiry
    jar = ActionController::CookieJar.new({})
    jar.signed.permanent[:session_token] = { value: "tok", httponly: true, same_site: :lax }
    jar.permanent.signed[:remember] = "me"
    assert_equal "tok", jar.signed[:session_token]
    assert_equal "me", jar.signed[:remember]
    refute_equal "", jar.flag_expires(:session_token)
    refute_equal "", jar.flag_expires(:remember)
    assert_equal "Lax", jar.flag_samesite(:session_token), "options still apply under permanent"
  end

  def test_only_permanent_writes_expire
    jar = ActionController::CookieJar.new({})
    jar[:plain] = "x"
    jar.signed[:signed_only] = "y"
    jar.permanent[:kept] = "z"
    assert_equal "", jar.flag_expires(:plain)
    assert_equal "", jar.flag_expires(:signed_only)
    jar.delete(:kept)
    assert_equal "", jar.flag_expires(:kept), "a deleted cookie is cleared, not kept for twenty years"
  end

  def test_bare_writes_default_httponly_and_honor_an_explicit_opt_out
    jar = ActionController::CookieJar.new({})
    jar[:last_room] = 7
    jar.signed[:session_token] = "abc123"
    assert jar.flag_httponly?(:last_room), "unsigned bare write keeps the HttpOnly default"
    assert jar.flag_httponly?(:session_token), "signed bare write keeps the HttpOnly default"
    jar.signed[:open_token] = { value: "tok", httponly: false }
    refute jar.flag_httponly?(:open_token)
    refute jar.flag_secure?(:last_room)
    jar.signed[:cross] = { value: "tok", same_site: :none, secure: true }
    assert_equal "None", jar.flag_samesite(:cross)
    assert jar.flag_secure?(:cross)
    assert jar.flag_httponly?(:cross), "options hash without httponly keeps the default"
    jar.signed[:strict] = { value: "tok", same_site: :strict }
    assert_equal "Strict", jar.flag_samesite(:strict)
    assert jar.flag_httponly?(:strict)
  end

  # NIL, as Rails answers — the signed read is the one nullable read in
  # this jar. `if token = cookies.signed[:session_token]` is campfire's
  # `SessionLookup`, and an empty String is TRUTHY in Ruby, so a `""`
  # here made a signed-out request take the signed-in branch and query
  # for `token: ""`. The UNSIGNED read below still answers `""`.
  def test_a_tampered_signed_cookie_reads_as_absent
    jar = ActionController::CookieJar.new({})
    jar.signed[:session_token] = "abc123"
    tampered = ActionController::CookieJar.new({"session_token" => jar["session_token"] + "x"})
    assert_nil tampered.signed[:session_token]
  end

  def test_a_cookie_signed_for_another_name_does_not_verify
    jar = ActionController::CookieJar.new({})
    jar.signed[:session_token] = "abc123"
    moved = ActionController::CookieJar.new({"other_token" => jar["session_token"]})
    assert_nil moved.signed[:other_token]
  end

  def test_an_absent_signed_cookie_is_nil_and_an_absent_plain_one_is_empty
    jar = ActionController::CookieJar.new({})
    assert_nil jar.signed[:nothing]
    assert_equal "", jar[:nothing]
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
