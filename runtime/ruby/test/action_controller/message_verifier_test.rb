require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_controller/message_verifier.rb`.
#
# RUBY-FAMILY ONLY, for the same reason cookies_test.rb beside it is:
# the verifier leans on MessageDigest's PBKDF2/HMAC, which only the
# ruby-family trees stage.
#
# THE VECTORS ARE REAL RAILS OUTPUT, not this file's own. Both signed
# strings below were produced by ActiveSupport 8.1.3:
#
#   key = ActiveSupport::KeyGenerator.new(SECRET,
#           hash_digest_class: OpenSSL::Digest::SHA256).generate_key(SALT, 64)
#   v   = ActiveSupport::MessageVerifier.new(key, digest: "SHA256",
#           serializer: JSON, url_safe: false)
#   v.generate(42, purpose: "user/avatar")
#   v.generate(42, purpose: "user/transfer", expires_at: Time.utc(2030,1,2,3,4,5))
#
# That is the whole point of this file. A round-trip test (generate then
# verify) passes just as happily on a format nobody else speaks, and a
# signed id campfire puts in a URL has to survive meeting the real Rails
# — the emit's tokens and Rails' are the same bytes or a migration
# invalidates every avatar link in the wild.
class ActionControllerMessageVerifierTest < Minitest::Test
  SECRET = "0123456789abcdef" * 8
  ID_SALT = ActiveRecord::SignedId::SALT

  RAILS_ID_42 =
    "eyJfcmFpbHMiOnsibWVzc2FnZSI6Ik5EST0iLCJleHAiOm51bGwsInB1ciI6InVzZXIvYXZhdGFyIn19" \
    "--8e7a7832b7842ab87f1fe1eeff03cef6e9513145a2587929a401a8993a429c88"

  RAILS_ID_42_EXPIRING =
    "eyJfcmFpbHMiOnsibWVzc2FnZSI6Ik5EST0iLCJleHAiOiIyMDMwLTAxLTAyVDAzOjA0OjA1LjAwMFoi" \
    "LCJwdXIiOiJ1c2VyL3RyYW5zZmVyIn19" \
    "--32be03acda91489627cf8c1fb81b4b983e7625c06c17a45c8288cf0594c69db8"

  # ── wire fidelity against real Rails ─────────────────────────

  def test_signed_id_envelope_is_byte_identical_to_rails
    assert_equal RAILS_ID_42,
                 ActionController::MessageVerifier.envelope(
                   SECRET, ID_SALT, "42", "user/avatar", "null", false
                 )
  end

  def test_expiring_envelope_is_byte_identical_to_rails
    exp = "\"" +
          ActionController::MessageVerifier.iso8601_ms(Time.utc(2030, 1, 2, 3, 4, 5)) +
          "\""
    assert_equal RAILS_ID_42_EXPIRING,
                 ActionController::MessageVerifier.envelope(
                   SECRET, ID_SALT, "42", "user/transfer", exp, false
                 )
  end

  def test_a_rails_minted_signed_id_verifies_here
    assert_equal "42",
                 ActionController::MessageVerifier.verified_json(
                   SECRET, ID_SALT, RAILS_ID_42, "user/avatar", false
                 )
  end

  # The message is a bare JSON Integer, NOT the quoted String a cookie's
  # is — `verified` (the cookie face) unquotes, so it must NOT be the
  # entry point a signed id reads.
  def test_the_signed_id_message_is_an_unquoted_integer
    payload = RAILS_ID_42.split("--")[0]
    envelope = Base64.strict_decode64(payload)
    message = ActionController::MessageVerifier.extract(envelope, "\"message\":\"")
    assert_equal "42", Base64.strict_decode64(message)
  end

  # ── rejection ────────────────────────────────────────────────

  def test_a_wrong_purpose_does_not_verify
    assert_equal "", ActionController::MessageVerifier.verified_json(
      SECRET, ID_SALT, RAILS_ID_42, "user/transfer", false
    )
  end

  def test_a_tampered_digest_does_not_verify
    tampered = RAILS_ID_42.sub(/.$/, "0")
    assert_equal "", ActionController::MessageVerifier.verified_json(
      SECRET, ID_SALT, tampered, "user/avatar", false
    )
  end

  def test_a_string_that_is_not_of_the_form_does_not_verify
    assert_equal "", ActionController::MessageVerifier.verified_json(
      SECRET, ID_SALT, "not-a-token", "user/avatar", false
    )
  end

  # `exp` is compared LEXICOGRAPHICALLY, which the iso8601_ms shape
  # makes chronological. 2030 is ahead of any run of this suite; a
  # past instant is not.
  def test_an_unexpired_signed_id_verifies
    assert_equal "42", ActionController::MessageVerifier.verified_json(
      SECRET, ID_SALT, RAILS_ID_42_EXPIRING, "user/transfer", false
    )
  end

  def test_an_expired_signed_id_does_not_verify
    exp = "\"" +
          ActionController::MessageVerifier.iso8601_ms(Time.utc(2001, 2, 3, 4, 5, 6)) +
          "\""
    stale = ActionController::MessageVerifier.envelope(
      SECRET, ID_SALT, "42", "user/transfer", exp, false
    )
    assert_equal "", ActionController::MessageVerifier.verified_json(
      SECRET, ID_SALT, stale, "user/transfer", false
    )
  end

  # ── the cookie face still round-trips ────────────────────────
  # `generate`/`verified` are now the String/no-exp view of the pair
  # above; the jar's behaviour must not have moved.

  def test_a_signed_cookie_round_trips_through_the_sha1_digest
    signed = ActionController::MessageVerifier.generate(
      SECRET, ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
      "abc", "cookie.session_token", true
    )
    assert_equal "abc", ActionController::MessageVerifier.verified(
      SECRET, ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
      signed, "cookie.session_token", true
    )
  end

  # ── iso8601_ms ───────────────────────────────────────────────

  def test_iso8601_ms_is_utc_with_three_fractional_digits
    assert_equal "2030-01-02T03:04:05.000Z",
                 ActionController::MessageVerifier.iso8601_ms(Time.utc(2030, 1, 2, 3, 4, 5))
  end
end
