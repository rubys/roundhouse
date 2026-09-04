require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_controller/message_verifier.rb`.
#
# RUBY-FAMILY ONLY, for the same reason cookies_test.rb beside it is:
# the verifier leans on MessageDigest's PBKDF2/HMAC, which only the
# ruby-family trees stage.
#
# THE VECTORS ARE A REAL RAILS APP'S OUTPUT, and the difference between
# that and "real ActiveSupport's" cost this file its whole point once.
# The earlier vectors were produced by assembling the pieces by hand —
#
#   key = ActiveSupport::KeyGenerator.new(SECRET,
#           hash_digest_class: OpenSSL::Digest::SHA256).generate_key(SALT, 64)
#   v   = ActiveSupport::MessageVerifier.new(key, digest: "SHA256",
#           serializer: JSON, url_safe: false)
#
# — which is genuine ActiveSupport and a configuration no Rails app has.
# `KeyGenerator.new` defaults to 65_536 iterations where an application's
# generator passes 1_000, and a hand-passed `serializer: JSON` produces a
# `message`/base64 envelope where the app's verifier produces `data`.
# Two wrong answers, both signed correctly, pinned as the truth.
#
# These come from campfire under Rails 8.2 (`oracle.json` records the
# revision), minted through the app:
#
#   SECRET_KEY_BASE=<SECRET> bin/rails runner '
#     u = User.find(1)
#     puts u.signed_id(purpose: :avatar)
#     puts u.signed_id(purpose: :transfer, expires_at: Time.utc(2030,1,2,3,4,5))
#     req = ActionDispatch::Request.new(Rails.application.env_config.merge(
#             "HTTP_HOST" => "127.0.0.1", "rack.input" => StringIO.new))
#     jar = ActionDispatch::Cookies::CookieJar.build(req, {})
#     jar.signed[:session_token] = { value: "hello", httponly: true }
#     puts jar[:session_token]'
#
# `scripts/campfire-oracle` builds that app; regenerate there rather than
# reasoning about the envelope. A round-trip test (generate then verify)
# passes just as happily on a format nobody else speaks, and a signed id
# campfire puts in a URL has to survive meeting the real Rails — the
# emit's tokens and Rails' are the same bytes or a migration invalidates
# every session and every avatar link in the wild.
class ActionControllerMessageVerifierTest < Minitest::Test
  # The 128-hex secret the vectors below were minted under. It is the
  # value of SECRET_KEY_BASE in the runner, verbatim — a shorter one
  # would derive a different key and every vector would be noise.
  SECRET = "0123456789abcdef" * 8
  ID_SALT = ActiveRecord::SignedId::SALT

  # `user.signed_id(purpose: :avatar)` for User#1. Note `"data":1` and
  # NO `exp` key: an unexpiring signed id omits the field rather than
  # writing null, which is the cookie jar's shape.
  RAILS_ID_1 =
    "eyJfcmFpbHMiOnsiZGF0YSI6MSwicHVyIjoidXNlci9hdmF0YXIifX0" \
    "--22322f8a68b987f03b1d1870c0229da751a69c957606ffff46631f4dfa45c0eb"

  RAILS_ID_1_EXPIRING =
    "eyJfcmFpbHMiOnsiZGF0YSI6MSwiZXhwIjoiMjAzMC0wMS0wMlQwMzowNDowNS4wMDBaIiwicHVyIjoi" \
    "dXNlci90cmFuc2ZlciJ9fQ" \
    "--e84549dcc587093e7832f3378faad650f7be99c696e151ad2e2ed92250c6ad12"

  # `cookies.signed[:session_token] = { value: "hello", … }` through the
  # app's own jar. The other envelope: base64 in `message`, `exp` always
  # present, HMAC-SHA1.
  RAILS_COOKIE_HELLO =
    "eyJfcmFpbHMiOnsibWVzc2FnZSI6IkltaGxiR3h2SWc9PSIsImV4cCI6bnVsbCwicHVyIjoiY29va2ll" \
    "LnNlc3Npb25fdG9rZW4ifX0=" \
    "--2c6b1537bd635fbd75b4b2e156c8957a0c03b154"

  # ── wire fidelity against real Rails ─────────────────────────

  def test_signed_id_envelope_is_byte_identical_to_rails
    assert_equal RAILS_ID_1,
                 ActionController::MessageVerifier.data_envelope(
                   SECRET, ID_SALT, "1", "user/avatar", "", false
                 )
  end

  def test_expiring_envelope_is_byte_identical_to_rails
    exp = "\"" +
          ActionController::MessageVerifier.iso8601_ms(Time.utc(2030, 1, 2, 3, 4, 5)) +
          "\""
    assert_equal RAILS_ID_1_EXPIRING,
                 ActionController::MessageVerifier.data_envelope(
                   SECRET, ID_SALT, "1", "user/transfer", exp, false
                 )
  end

  # THE OTHER HALF, and the one the cable sweep needs: a cookie minted
  # by the app has to verify here, because the benchmark hands one file
  # of pre-minted `session_token`s to Rails and to the binary. Before the
  # iteration count was measured (65_536, an application never uses it)
  # each side rejected the other's cookie.
  def test_the_signed_cookie_envelope_is_byte_identical_to_rails
    assert_equal RAILS_COOKIE_HELLO,
                 ActionController::MessageVerifier.generate(
                   SECRET, ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
                   "hello", "cookie.session_token", true
                 )
  end

  def test_a_rails_minted_cookie_verifies_here
    assert_equal "hello",
                 ActionController::MessageVerifier.verified(
                   SECRET, ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
                   RAILS_COOKIE_HELLO, "cookie.session_token", true
                 )
  end

  def test_a_rails_minted_signed_id_verifies_here
    assert_equal "1",
                 ActionController::MessageVerifier.verified_data_json(
                   SECRET, ID_SALT, RAILS_ID_1, "user/avatar", false
                 )
  end

  # The id is a bare JSON Integer in a `data` field, NOT base64 in a
  # `message` one — so `verified_json` (the cookie face) must not be the
  # entry point a signed id reads, and it would find no `message` if it
  # were.
  def test_the_signed_id_carries_an_unquoted_integer_in_data
    payload = RAILS_ID_1.split("--")[0]
    envelope = Base64.urlsafe_decode64(payload)
    assert_equal "1", ActionController::MessageVerifier.extract_raw(envelope, "\"data\":")
    assert_equal "", ActionController::MessageVerifier.extract(envelope, "\"message\":\"")
  end

  # ── rejection ────────────────────────────────────────────────

  def test_a_wrong_purpose_does_not_verify
    assert_equal "", ActionController::MessageVerifier.verified_data_json(
      SECRET, ID_SALT, RAILS_ID_1, "user/transfer", false
    )
  end

  def test_a_tampered_digest_does_not_verify
    tampered = RAILS_ID_1.sub(/.$/, "0")
    assert_equal "", ActionController::MessageVerifier.verified_data_json(
      SECRET, ID_SALT, tampered, "user/avatar", false
    )
  end

  def test_a_string_that_is_not_of_the_form_does_not_verify
    assert_equal "", ActionController::MessageVerifier.verified_data_json(
      SECRET, ID_SALT, "not-a-token", "user/avatar", false
    )
  end

  # `exp` is compared LEXICOGRAPHICALLY, which the iso8601_ms shape
  # makes chronological. 2030 is ahead of any run of this suite; a
  # past instant is not.
  def test_an_unexpired_signed_id_verifies
    assert_equal "1", ActionController::MessageVerifier.verified_data_json(
      SECRET, ID_SALT, RAILS_ID_1_EXPIRING, "user/transfer", false
    )
  end

  def test_an_expired_signed_id_does_not_verify
    exp = "\"" +
          ActionController::MessageVerifier.iso8601_ms(Time.utc(2001, 2, 3, 4, 5, 6)) +
          "\""
    stale = ActionController::MessageVerifier.data_envelope(
      SECRET, ID_SALT, "1", "user/transfer", exp, false
    )
    assert_equal "", ActionController::MessageVerifier.verified_data_json(
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
