# ActiveRecord::TokenFor — the tokens `generates_token_for` mints, here
# for the one declaration every Rails 8 app carries without writing it:
# `has_secure_password`'s password-reset token (`reset_token: true` is
# its default). The authentication generator's mailer puts
# `user.password_reset_token` in the reset link, and its
# PasswordsController reads it back with `find_by_password_reset_token!`.
#
# The wire format is ActiveSupport's, and it is the SIGNED-GLOBALID one
# (`MessageVerifier.gid_envelope`): url-safe base64 WITH padding, HMAC-SHA1
# (`Rails.application.message_verifiers` passes no digest), the payload in
# a `data` field. Only the salt differs. MEASURED under Rails 8.1.4,
# `SECRET_KEY_BASE=test-secret`, a User with id 1:
#
#   {"_rails":{"data":[1,"Iyo3zOdjGO"],"exp":"2026-10-03T20:10:58.951Z","pur":"User\npassword_reset\n900"}}
#   …--5f96b3553eb0ad2afc31948cd14f57df8af251e4
#
# reproduced at PBKDF2(secret, "active_record/token_for", 1_000, 64) +
# HMAC-SHA1 over the padded payload. The data is `[id, block value]`, and
# has_secure_password's block is `password_salt&.last(10)`: the last ten
# characters of the bcrypt salt, so a token dies when the password
# changes. The purpose is `"<Model>\n<purpose>\n<expires_in seconds>"`.
#
# What arrives here is already specific to the model: the purpose and
# the expiry are compile-time facts the lowering writes at the call site
# (src/lower/secure_password.rs), like signed_id.rb's combined purpose.
# The purpose arrives JSON-ESCAPED (`User\npassword_reset\n900` with a
# literal backslash-n), because the envelope is written and read as text.
#
# A reopen-tier file like signed_id.rb: the strict targets do not stage
# it, since the PBKDF2/HMAC primitives ship only with the ruby family.
module ActiveRecord
  module TokenFor
    SALT = "active_record/token_for"

    # The signed token for `data_json` under `purpose`, expiring
    # `expires_in` seconds from now.
    def self.generate(data_json, purpose, expires_in)
      exp = "\"" +
            ActionController::MessageVerifier.iso8601_ms(Time.now + expires_in) +
            "\""
      ActionController::MessageVerifier.gid_envelope(
        Rails.application.secret_key_base, SALT, data_json, purpose, exp
      )
    end

    # The `data` JSON `token` carries, or "" for every rejection:
    # tampered, signed for another purpose, or expired.
    def self.verified_data(token, purpose)
      ActionController::MessageVerifier.verified_data_json(
        Rails.application.secret_key_base, SALT, token, purpose, true
      )
    end

    # has_secure_password's payload: `[id, password_salt&.last(10)]` as
    # JSON. A bcrypt digest is `$2a$12$` + 22 salt characters + the hash,
    # so the salt's last ten are characters 19..28 of the digest.
    def self.secure_password_data(id, digest)
      tail = "null"
      if !digest.nil? && digest.length >= 29
        tail = ActionController::MessageVerifier.json_string(digest[19, 10])
      end
      "[" + id.to_s + "," + tail + "]"
    end

    # The record id at the head of a `[id, …]` payload, or 0 (no row)
    # for "" — the rejection `verified_data` answers.
    def self.data_id(data)
      return 0 if data.length < 2
      data[1, data.length - 1].to_i
    end
  end
end
