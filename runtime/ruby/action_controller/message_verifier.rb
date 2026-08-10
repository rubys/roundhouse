# ActiveSupport::MessageVerifier, reproduced at the wire level.
#
# This exists so a signed cookie the emitted app mints is one the
# reference Rails accepts, and vice versa. That matters twice: the
# differential test boots both and sends one cookie to each, and a real
# migration keeps every session and every signed id already in the wild
# (campfire puts `signed_id`s in URLs — `avatar_token` on every message).
#
# The format, measured against ActiveSupport 8.2 rather than inferred:
#
#   cookie  = strict_base64(envelope) + "--" + hex_digest
#   envelope = {"_rails":{"message":<strict_base64(json)>,"exp":null,
#                         "pur":"cookie.<name>"}}
#   digest  = HMAC(derived_key, strict_base64(envelope))
#   key     = PBKDF2-HMAC-SHA256(secret_key_base, salt, 2**16, 64)
#
# The `_rails` envelope is `use_message_serializer_for_metadata`, a 7.1
# default, and it is INSIDE the signed payload — the digest covers the
# base64 text, not the JSON.
#
# Rails uses two different digests, which is the one thing here most
# likely to look like a bug:
#
#   signed cookies   HMAC-SHA1    (`signed_cookie_digest || "SHA1"`, and
#                                 no load_defaults version sets
#                                 `cookies_digest` — 8.2 included)
#   signed ids       HMAC-SHA256  (`use_legacy_signed_id_verifier` is
#                                 `:generate_and_verify` by default, and
#                                 that path passes digest: "SHA256")
#
# Salts are Rails' defaults: "signed cookie" for the cookie jar,
# "active_record/signed_id" for signed ids. An app that overrides
# `config.action_dispatch.signed_cookie_salt` or `cookies_digest` would
# need those lifted at ingest; none of the corpus does.
module ActionController
  module MessageVerifier
    ITERATIONS = 65_536
    KEY_SIZE = 64
    SIGNED_COOKIE_SALT = "signed cookie"

    # PBKDF2 is deliberately expensive (2**16 iterations of HMAC), so a
    # derived key is worth keeping: a request that reads the session
    # cookie and then writes it back would otherwise pay twice.
    def self.derived_keys
      @derived_keys ||= {}
    end

    def self.derive_key(secret, salt)
      # Keyed by a separator neither part contains: the salts are
      # framework constants ("signed cookie", "active_record/signed_id")
      # and the secret is hex.
      cache_key = salt + "|" + secret
      # `key?` then read, rather than binding the read and testing it
      # for nil — the same idiom the cookie jar uses, and it keeps a
      # nilable local off spinel's `const char *` path.
      return derived_keys[cache_key] if derived_keys.key?(cache_key)
      key = MessageDigest.pbkdf2_sha256(secret, salt, ITERATIONS, KEY_SIZE)
      derived_keys[cache_key] = key
      key
    end

    # The signed string for `value` under `purpose`. `sha1` selects the
    # cookie digest; signed ids pass false for the SHA-256 one.
    def self.generate(secret, salt, value, purpose, sha1)
      message = Base64.strict_encode64(json_string(value))
      envelope = "{\"_rails\":{\"message\":\"" + message +
                 "\",\"exp\":null,\"pur\":\"" + purpose + "\"}}"
      payload = Base64.strict_encode64(envelope)
      payload + "--" + digest_for(secret, salt, payload, sha1)
    end

    # The value carried by `signed`, or "" when the signature does not
    # verify, the purpose does not match, or the shape is not ours. A
    # tampered cookie is indistinguishable from an absent one, which is
    # what Rails does too (it returns nil and the app treats it as signed
    # out).
    def self.verified(secret, salt, signed, purpose, sha1)
      sep = signed.index("--")
      return "" if sep.nil?
      payload = signed[0, sep]
      supplied = signed[sep + 2, signed.length - sep - 2]
      return "" if supplied != digest_for(secret, salt, payload, sha1)
      envelope = Base64.strict_decode64(payload)
      return "" if extract(envelope, "\"pur\":\"") != purpose
      message = extract(envelope, "\"message\":\"")
      return "" if message == ""
      json_value(Base64.strict_decode64(message))
    end

    def self.digest_for(secret, salt, payload, sha1)
      key = derive_key(secret, salt)
      return MessageDigest.hmac_sha1_hex(key, payload) if sha1
      MessageDigest.hmac_sha256_hex(key, payload)
    end

    # The envelope is a fixed shape this file also writes, so the two
    # string fields it carries are read by scanning rather than by
    # parsing JSON — the ruby-family runtimes ship no parser, and a
    # parser sized for one known object is more surface than the scan.
    # A field that is absent (or not a plain string) reads as "".
    def self.extract(envelope, prefix)
      at = envelope.index(prefix)
      return "" if at.nil?
      rest = at + prefix.length
      close = envelope.index("\"", rest)
      return "" if close.nil?
      envelope[rest, close - rest]
    end

    # The message is JSON, and every value the cookie jar signs is a
    # String, so the serializer round-trip is quote-wrapping. A
    # non-string message (Rails would allow one) is handed back verbatim
    # rather than guessed at.
    def self.json_string(value)
      "\"" + value.to_s + "\""
    end

    def self.json_value(json)
      return json if json.length < 2
      return json if json[0, 1] != "\""
      json[1, json.length - 2]
    end
  end
end
