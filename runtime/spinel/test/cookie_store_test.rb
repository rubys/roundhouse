# frozen_string_literal: true
# Minitest-shaped, like request_forgery_protection_test.rb: a CRuby-only
# framework test of the ruby family's session cookie (runtime/
# cookie_store.rb; `cookies.encrypted` itself is in
# encrypted_cookie_jar_test.rb). A session survives its own round trip
# with its value types and its flash; anything the client could have sent instead
# (an edit to any of the three parts, a cookie for another name or under
# another secret, an expired one, the old signed `k=v` cookie, garbage)
# reads as an empty session, as Rails reads it.
require "minitest/autorun"
require_relative "test_helper"
require_relative "../runtime/cookie_store"

class CookieStoreTest < Minitest::Test
  NAME = "_campfire_session"

  def secret = Rails.application.secret_key_base

  def session_with(pairs)
    s = ActionDispatch::Session.new
    pairs.each { |k, v| s[k] = v }
    s
  end

  def written(pairs, name = NAME)
    ActionDispatch::CookieStore.cookie_value(session_with(pairs), name)
  end

  def restored(raw, name = NAME)
    ActionDispatch::CookieStore.restore(raw, name).to_hash
  end

  def test_a_session_round_trips_with_its_value_types
    raw = written("session_id" => "0" * 32, "authenticated_at" => 1760000000, "user" => "ada",
                  "prefs" => { "theme" => "dark", "size" => 3 }, "tags" => ["a", 1, nil, true])
    assert_equal({ "session_id" => "0" * 32, "authenticated_at" => 1760000000, "user" => "ada",
                   "prefs" => { "theme" => "dark", "size" => 3 }, "tags" => ["a", 1, nil, true] }, restored(raw))
  end

  def test_a_written_session_gets_a_session_id_first
    data = restored(written("user" => "ada"))
    assert_equal ["session_id", "user"], data.keys
    assert_match(/\A[0-9a-f]{32}\z/, data["session_id"])
    assert_equal "kept", restored(written("user" => "ada", "session_id" => "kept"))["session_id"]
  end

  def test_the_cookie_is_three_strict_base64_parts_and_hides_the_session
    raw = written("_csrf_token" => "abc")
    ct, iv, tag = raw.split("--")
    assert_equal 3, raw.split("--").length
    assert_equal 12, Base64.strict_decode64(iv).bytesize
    assert_equal 16, Base64.strict_decode64(tag).bytesize
    refute_includes Base64.strict_decode64(ct), "_csrf_token"
    refute_equal raw, written("_csrf_token" => "abc"), "a fresh IV per write"
  end

  def test_an_absent_garbled_or_old_signed_cookie_restores_empty
    assert_equal({}, restored(""))
    assert_equal({}, restored("garbage"))
    assert_equal({}, restored("_csrf_token=planted"))
    old = ActionController::MessageVerifier.generate(secret, ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
                                                     "_csrf_token=abc", "cookie." + NAME, true)
    assert_equal({}, restored(old))
  end

  def test_any_edit_restores_empty
    raw = written("user" => "ada")
    ct, iv, tag = raw.split("--")
    flip = ->(part) { (part[0] == "A" ? "B" : "A") + part[1..] }
    assert_equal({}, restored([flip.(ct), iv, tag].join("--")))
    assert_equal({}, restored([ct, flip.(iv), tag].join("--")))
    assert_equal({}, restored([ct, iv, flip.(tag)].join("--")))
    assert_equal({}, restored([ct, iv].join("--")))
    assert_equal({}, restored([ct, iv, tag[0, 20]].join("--")))
  end

  def test_a_cookie_for_another_name_or_secret_restores_empty
    assert_equal({}, restored(written("user" => "ada"), "_other_session"))
    other = ActionController::MessageEncryptor.encrypt("another-secret", "{\"user\":\"ada\"}", "cookie." + NAME, "null")
    assert_equal({}, restored(other))
  end

  def test_an_expired_envelope_restores_empty
    raw = ActionController::MessageEncryptor.encrypt(secret, "{\"user\":\"ada\"}", "cookie." + NAME,
                                                     "\"2000-01-01T00:00:00.000Z\"")
    assert_equal({}, restored(raw))
    raw = ActionController::MessageEncryptor.encrypt(secret, "{\"user\":\"ada\"}", "cookie." + NAME,
                                                     "\"2999-01-01T00:00:00.000Z\"")
    assert_equal({ "user" => "ada" }, restored(raw))
  end

  # A non-ASCII flash message ("✓") and the HTML-sensitive characters
  # ActiveSupport's JSON escapes.
  def test_the_flash_rides_in_the_session_and_shows_once
    flash = ActionDispatch::Flash.new
    flash[:notice] = %(Saved ✓ <b>"&"</b>)
    session = ActionDispatch::CookieStore.commit_flash(session_with("user" => "ada"), flash)
    assert_equal({ "discard" => [], "flashes" => { "notice" => %(Saved ✓ <b>"&"</b>) } }, session["flash"])

    back = ActionDispatch::CookieStore.restore(ActionDispatch::CookieStore.cookie_value(session, NAME), NAME)
    inbound = ActionDispatch::CookieStore.inbound_flash(back)
    assert_equal({ "notice" => %(Saved ✓ <b>"&"</b>) }, inbound)
    shown = ActionDispatch::Flash.new(inbound)
    ActionDispatch::CookieStore.commit_flash(back, shown)
    refute back.key?("flash"), "a message carried in and shown is not carried on"
  end

  def test_the_discard_list_and_unknown_flash_keys_are_not_shown
    session = session_with("flash" => { "discard" => ["alert"], "flashes" => { "notice" => "n", "alert" => "a", "error" => "e" } })
    assert_equal({ "notice" => "n" }, ActionDispatch::CookieStore.inbound_flash(session))
    assert_equal({}, ActionDispatch::CookieStore.inbound_flash(session_with("flash" => "not a flash")))
  end
end
