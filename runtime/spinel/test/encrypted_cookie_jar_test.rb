# frozen_string_literal: true
# Minitest-shaped, like request_forgery_protection_test.rb: a CRuby-only
# framework test of `cookies.encrypted` (ActionController::EncryptedCookieJar
# over runtime/ruby/action_controller/message_encryptor.rb). A value survives
# its own round trip with its type; anything the client could have sent
# instead (an edit to any of the three parts, a cookie for another name or
# under another secret, an expired one, garbage) reads as nil, as Rails
# reads it.
require "minitest/autorun"
require_relative "test_helper"

class EncryptedCookieJarTest < Minitest::Test
  NAME = "prefs"

  def secret = Rails.application.secret_key_base

  def written(value, name = NAME)
    jar = ActionController::CookieJar.new
    jar.encrypted[name] = value
    jar[name]
  end

  def read(raw, name = NAME)
    ActionController::CookieJar.new(name => raw).encrypted[name]
  end

  def test_a_value_round_trips_with_its_type
    # A Hash is written through the options form, as in Rails: a bare Hash
    # IS the options, and without :value it stores nil (actionpack 8.1.4).
    value = { "theme" => "dark", "size" => 3, "tags" => ["a", 1, nil, true] }
    assert_equal value, read(written({ value: value }))
    assert_nil read(written(value))
    assert_equal 42, read(written(42))
    assert_equal "ada", read(written("ada"))
  end

  def test_the_cookie_is_three_strict_base64_parts_and_hides_the_value
    raw = written("secret-value")
    ct, iv, tag = raw.split("--")
    assert_equal 3, raw.split("--").length
    assert_equal 12, Base64.strict_decode64(iv).bytesize
    assert_equal 16, Base64.strict_decode64(tag).bytesize
    refute_includes Base64.strict_decode64(ct), "secret-value"
    refute_equal raw, written("secret-value"), "a fresh IV per write"
  end

  def test_an_absent_or_garbled_cookie_reads_nil
    assert_nil ActionController::CookieJar.new.encrypted[NAME]
    assert_nil read("garbage")
    assert_nil read("a--b--c")
  end

  def test_any_edit_reads_nil
    raw = written("ada")
    ct, iv, tag = raw.split("--")
    flip = ->(part) { (part[0] == "A" ? "B" : "A") + part[1..] }
    assert_nil read([flip.(ct), iv, tag].join("--"))
    assert_nil read([ct, flip.(iv), tag].join("--"))
    assert_nil read([ct, iv, flip.(tag)].join("--"))
    assert_nil read([ct, iv].join("--"))
    assert_nil read([ct, iv, tag[0, 20]].join("--"))
  end

  def test_a_cookie_for_another_name_or_secret_reads_nil
    assert_nil read(written("ada", "other"), NAME)
    other = ActionController::MessageEncryptor.encrypt("another-secret", "\"ada\"", "cookie." + NAME, "null")
    assert_nil read(other)
  end

  def test_an_expired_envelope_reads_nil
    raw = ActionController::MessageEncryptor.encrypt(secret, "\"ada\"", "cookie." + NAME, "\"2000-01-01T00:00:00.000Z\"")
    assert_nil read(raw)
    raw = ActionController::MessageEncryptor.encrypt(secret, "\"ada\"", "cookie." + NAME, "\"2999-01-01T00:00:00.000Z\"")
    assert_equal "ada", read(raw)
  end

  def test_the_options_hash_form_writes_its_value
    jar = ActionController::CookieJar.new
    jar.encrypted[:prefs] = { value: { "theme" => "dark", "n" => 2 }, httponly: true }
    jar.encrypted[:count] = 42
    reread = ActionController::CookieJar.new(jar.pending)
    assert_equal({ "theme" => "dark", "n" => 2 }, reread.encrypted[:prefs])
    assert_equal 42, reread.encrypted["count"]
    assert_nil reread.encrypted[:missing]
    assert_nil ActionController::CookieJar.new("count" => reread["prefs"]).encrypted[:count], "written for another name"
  end

  def test_a_permanent_encrypted_write_expires_twenty_years_out
    jar = ActionController::CookieJar.new
    jar.encrypted.permanent[:token] = "t"
    assert_equal "t", ActionController::CookieJar.new(jar.pending).encrypted[:token]
    refute_equal "", jar.flag_expires(:token)
    ct, iv, tag = jar["token"].split("--")
    key = ActionController::MessageEncryptor.derive_key(secret)
    plain = MessageDigest.aes_256_gcm_decrypt(key, Base64.strict_decode64(iv), Base64.strict_decode64(ct), Base64.strict_decode64(tag))
    assert_match(/"exp":"#{Time.now.utc.year + 20}-/, plain)
  end
end
