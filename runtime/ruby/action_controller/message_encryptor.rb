# frozen_string_literal: true
# ActiveSupport::MessageEncryptor as Rails' encrypted cookie jar uses it,
# reproduced at the wire level: `cookies.encrypted`, and the session
# store, which writes the session through that jar.
#
# The format, measured against Rails 8.1.4 (`load_defaults 8.1`):
#
#   cookie = strict_base64(ciphertext) + "--" + strict_base64(iv) +
#            "--" + strict_base64(auth_tag)
#   cipher = AES-256-GCM, 12-byte IV, 16-byte tag, no associated data
#   key    = PBKDF2-HMAC-SHA256(secret_key_base,
#            "authenticated encrypted cookie", 1_000, 32)
#   plain  = the signed jar's `_rails` envelope (MessageVerifier
#            .message_envelope): the message JSON base64ed into
#            `message`, then `exp` and `pur` ("cookie.<name>")
#
# A fresh random IV per write, so the same value encrypts differently
# every time; callers that want to know whether a value changed compare
# the plain JSON, not the cookie.
#
# Reading follows Rails' cookie jar, which tries twice
# (`AbstractCookieJar#[]`): with the purpose `cookie.<name>`, then, if that
# answers nothing, with no purpose at all. The second read is what accepts
# a cookie written before cookies carried metadata (Rails < 5.2: plaintext
# that is the bare JSON, no envelope), under any name; an envelope that
# names a purpose never passes it. Everything else reads as "", which the
# callers answer as an absent cookie, as Rails answers nil: a value that is
# not three strict-base64 parts of the right lengths, a tag that does not
# authenticate (a tampered ciphertext, IV or tag, another secret), a
# purpose that is not this cookie's, an `exp` already past.
#
# NOT READ, where Rails reads it: the `{"_rails":{"data":…}}` envelope,
# which a MessageEncryptor with a nesting serializer writes and no cookie
# jar does; key rotation (`config.action_dispatch.cookies_rotations`); the
# AES-256-CBC jar (`use_authenticated_cookie_encryption = false`); and an
# app that overrides `authenticated_encrypted_cookie_salt`,
# `encrypted_cookie_cipher` or `cookies_serializer`. Those are read
# nowhere in this runtime, so an app that sets them is not one this file
# answers for (runtime.md, "`cookies.encrypted` is Rails' encrypted cookie jar").
module ActionController
  module MessageEncryptor
    SALT = "authenticated encrypted cookie"
    KEY_SIZE = 32
    IV_SIZE = 12
    TAG_SIZE = 16
    # strict_base64 of 12 and of 16 bytes: Rails splits the cookie from the
    # right at these lengths (`extract_parts`), not at every "--".
    ENCODED_IV_LENGTH = 16
    ENCODED_TAG_LENGTH = 24
    # How Rails recognizes the `message` envelope inside the plaintext
    # (`dual_serialized_metadata_envelope_json?`).
    ENVELOPE_PREFIX = "{\"_rails\":{\"message\":"

    # Derived once per secret, as MessageVerifier keeps its keys: PBKDF2
    # is 1_000 HMACs, and a request reads the session and may write it.
    def self.derived_keys
      @derived_keys ||= {}
    end

    def self.derive_key(secret)
      return derived_keys[secret] if derived_keys.key?(secret)
      key = MessageDigest.pbkdf2_sha256(secret, SALT, ActionController::MessageVerifier::ITERATIONS, KEY_SIZE)
      derived_keys[secret] = key
      key
    end

    # The cookie value for `message_json` under `purpose`. `exp` is the
    # JSON for the envelope's `exp` slot: "null", or a quoted
    # `MessageVerifier.iso8601_ms` instant.
    def self.encrypt(secret, message_json, purpose, exp)
      plain = ActionController::MessageVerifier.message_envelope(message_json, purpose, exp)
      iv = MessageDigest.secure_random_bytes(IV_SIZE)
      sealed = MessageDigest.aes_256_gcm_encrypt(derive_key(secret), iv, plain)
      n = sealed.bytesize - TAG_SIZE
      Base64.strict_encode64(sealed.byteslice(0, n).to_s) + "--" +
        Base64.strict_encode64(iv) + "--" +
        Base64.strict_encode64(sealed.byteslice(n, TAG_SIZE).to_s)
    end

    # The message JSON `value` carries for `purpose`, or "" for every
    # rejection (see the header). The caller parses it.
    def self.decrypted_json(secret, value, purpose)
      plain = decrypt(secret, value)
      return "" if plain == ""
      json = purposed_json(plain, purpose)
      return json if json != ""
      unpurposed_json(plain)
    end

    # The first read: a `message` envelope for exactly this purpose.
    # Rails' `deserialize_with_metadata` recognizes that envelope by its
    # opening bytes; plaintext without metadata is "missing metadata" when
    # a purpose is expected.
    def self.purposed_json(plain, purpose)
      return "" unless plain.start_with?(ENVELOPE_PREFIX)
      ActionController::MessageVerifier.envelope_message(plain, purpose)
    end

    # The second read, with no purpose: an envelope must then name none,
    # and plaintext without one is the message itself.
    def self.unpurposed_json(plain)
      return ActionController::MessageVerifier.envelope_message(plain, "") if plain.start_with?(ENVELOPE_PREFIX)
      return "" if plain.start_with?("{\"_rails\":")
      plain
    end


    # The plaintext inside `value`, or "". Split from the right at the
    # encoded tag and IV lengths and decoded strictly, as Rails does: a
    # part that is not canonical base64 rejects the cookie before the
    # cipher runs.
    def self.decrypt(secret, value)
      n = value.length
      return "" if n < ENCODED_TAG_LENGTH + ENCODED_IV_LENGTH + 4
      tag_at = n - ENCODED_TAG_LENGTH
      return "" if value[tag_at - 2, 2] != "--"
      iv_at = tag_at - 2 - ENCODED_IV_LENGTH
      return "" if value[iv_at - 2, 2] != "--"
      ct = strict_decode(value[0, iv_at - 2].to_s)
      iv = strict_decode(value[iv_at, ENCODED_IV_LENGTH].to_s)
      tag = strict_decode(value[tag_at, ENCODED_TAG_LENGTH].to_s)
      return "" if ct == "" || iv.bytesize != IV_SIZE || tag.bytesize != TAG_SIZE
      MessageDigest.aes_256_gcm_decrypt(derive_key(secret), iv, ct, tag)
    end

    # `Base64.strict_decode64`, which raises on anything but canonical
    # base64; "" where it would raise. The runtime's own decoder skips
    # what it does not recognize, so the decoded bytes are re-encoded and
    # compared: only canonical text survives that. (The rescue is for the
    # stdlib's decoder, which a CRuby tree may have loaded over it.)
    def self.strict_decode(text)
      bytes = Base64.strict_decode64(text)
      return "" if Base64.strict_encode64(bytes) != text
      bytes
    rescue ArgumentError
      ""
    end
  end
end
