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
#   digest  = HMAC(derived_key, strict_base64(envelope))
#   key     = PBKDF2-HMAC-SHA256(secret_key_base, salt, 1_000, 64)
#
# TWO ENVELOPES, and which one a caller gets depends on its verifier's
# serializer rather than on anything the caller says:
#
#   signed cookie  {"_rails":{"message":<strict_base64(json)>,"exp":null,
#                             "pur":"cookie.<name>"}}
#   signed id      {"_rails":{"data":<json>,"pur":"<model>/<purpose>"}}
#                  {"_rails":{"data":<json>,"exp":"<iso8601_ms>",
#                             "pur":"<model>/<purpose>"}}
#
# and they are base64'd differently too: a cookie is strict base64, a
# signed id is URL-safe and unpadded (the verifier is `url_safe` because
# the token goes in a path segment).
#
# The cookie jar hands its verifier an already-serialized String, so
# `Messages::Metadata` cannot nest the metadata inside the payload and
# base64s it into a `message` field with an always-present `exp`. The
# signed-id verifier's serializer CAN, so the id goes in verbatim as
# `data` and `exp` is OMITTED ENTIRELY when there is none. Both measured
# against campfire under Rails 8.2, minting through the app's own
# `signed_id` and `CookieJar.build` — not through a hand-assembled
# `MessageVerifier`, which produces a third shape no app ever emits and
# which this file was pinned to for a while.
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
    # 1_000, WHICH IS THE APPLICATION'S NUMBER AND NOT THE CLASS'S.
    # `ActiveSupport::KeyGenerator.new(secret)` defaults to 2**16, and
    # this file said 65_536 for that reason — but no Rails app ever
    # constructs one that way. `Rails::Application#key_generator` passes
    # `iterations: 1000` explicitly, and every signed cookie and signed
    # id in a Rails app derives through it.
    #
    # MEASURED, not read: a `session_token` cookie minted by campfire
    # under Rails 8.2 (`ActionDispatch::Cookies::CookieJar.build` against
    # the app's own env_config) reproduces bit for bit at
    # PBKDF2-HMAC-SHA256(secret, "signed cookie", 1_000, 64) + HMAC-SHA1,
    # and at no other point in the {1_000, 65_536} x {SHA1, SHA256}^2
    # grid. At 65_536 the emitted app and Rails rejected each other's
    # cookies in both directions — the exact opposite of what the top of
    # this file promises, and it went unnoticed because both lanes of
    # every test we run derive the key HERE.
    #
    # ONCE's own load harness had the answer all along:
    # `test/performance/create_dummy_cookies.rb` forges its 10,000
    # cookies with `KeyGenerator.new("dummy", iterations: 1000)`.
    ITERATIONS = 1_000
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
    #
    # A cookie's message is a String and never expires, so this is the
    # String/no-exp face of `envelope` below.
    def self.generate(secret, salt, value, purpose, sha1)
      envelope(secret, salt, json_string(value), purpose, "null", sha1)
    end

    # The general form, for callers that serialize the message
    # themselves. `message_json` is the JSON of the payload — a signed
    # id's is a bare Integer (`123`), NOT the quoted `"123"` a String
    # message produces, and getting that wrong is invisible until a
    # token minted here meets a real Rails. `exp` is the JSON for the
    # envelope's `exp` slot: "null", or a quoted `iso8601_ms` instant.
    def self.envelope(secret, salt, message_json, purpose, exp, sha1)
      message = Base64.strict_encode64(message_json)
      env = "{\"_rails\":{\"message\":\"" + message +
            "\",\"exp\":" + exp + ",\"pur\":\"" + purpose + "\"}}"
      payload = Base64.strict_encode64(env)
      payload + "--" + digest_for(secret, salt, payload, sha1)
    end

    # ISO8601 in UTC with exactly three fractional digits — the shape
    # `ActiveSupport::Messages::Metadata` writes into `exp`
    # (`expires_at.utc.iso8601(3)`). That form is fixed-width and
    # zero-padded in a single zone, so LEXICOGRAPHIC order on it is
    # chronological order: expiry is a string compare and this runtime
    # needs no date parser to check one.
    def self.iso8601_ms(t)
      u = t.utc
      f = u.to_f
      ms = ((f - f.to_i) * 1000).to_i
      format(
        "%04d-%02d-%02dT%02d:%02d:%02d.%03dZ",
        u.year, u.mon, u.mday, u.hour, u.min, u.sec, ms
      )
    end

    # The value carried by `signed`, or "" when the signature does not
    # verify, the purpose does not match, or the shape is not ours. A
    # tampered cookie is indistinguishable from an absent one, which is
    # what Rails does too (it returns nil and the app treats it as signed
    # out).
    def self.verified(secret, salt, signed, purpose, sha1)
      json = verified_json(secret, salt, signed, purpose, sha1)
      return "" if json == ""
      json_value(json)
    end

    # The message JSON carried by `signed`, or "" for every rejection:
    # bad shape, bad signature, wrong purpose, or an `exp` already
    # past. Callers that signed a non-String message read this and
    # deserialize themselves.
    def self.verified_json(secret, salt, signed, purpose, sha1)
      sep = signed.index("--")
      return "" if sep.nil?
      payload = signed[0, sep]
      supplied = signed[sep + 2, signed.length - sep - 2]
      return "" if supplied != digest_for(secret, salt, payload, sha1)
      env = Base64.strict_decode64(payload)
      return "" if extract(env, "\"pur\":\"") != purpose
      # `"exp":null` does not match the quoted prefix, so an
      # unexpiring message reads "" here and skips the compare — which
      # is every signed cookie this file writes.
      exp = extract(env, "\"exp\":\"")
      return "" if exp != "" && exp <= iso8601_ms(Time.now)
      message = extract(env, "\"message\":\"")
      return "" if message == ""
      Base64.strict_decode64(message)
    end

    # The SIGNED-ID form: the value goes in as JSON, not base64, and an
    # absent expiry is an absent KEY rather than `"exp":null`. Separate
    # from `envelope` above rather than a flag on it, because the two
    # shapes have different fields in a different order and a caller that
    # picked the wrong one would mint a token that verifies here and
    # nowhere else — which is exactly the bug this pair replaces.
    def self.data_envelope(secret, salt, data_json, purpose, exp, sha1)
      env = "{\"_rails\":{\"data\":" + data_json
      env = env + ",\"exp\":" + exp if exp != ""
      env = env + ",\"pur\":\"" + purpose + "\"}}"
      # URL-SAFE AND UNPADDED, which is the other thing a signed id does
      # differently: `ActiveRecord::SignedId`'s verifier is `url_safe`,
      # because the token goes in a path segment (campfire's
      # `route_for :user_avatar, user.avatar_token`). The cookie jar's is
      # not. Measured: Rails' avatar token ends `…YXIifX0` where strict
      # base64 of the same envelope ends `…YXIifX0=`.
      payload = Base64.urlsafe_encode64_nopad(env)
      payload + "--" + digest_for(secret, salt, payload, sha1)
    end

    # The `data` a signed id carries, as JSON text, or "" for every
    # rejection — the mirror of `verified_json` for the other envelope.
    def self.verified_data_json(secret, salt, signed, purpose, sha1)
      sep = signed.index("--")
      return "" if sep.nil?
      payload = signed[0, sep]
      supplied = signed[sep + 2, signed.length - sep - 2]
      return "" if supplied != digest_for(secret, salt, payload, sha1)
      env = Base64.urlsafe_decode64(payload)
      return "" if extract(env, "\"pur\":\"") != purpose
      exp = extract(env, "\"exp\":\"")
      return "" if exp != "" && exp <= iso8601_ms(Time.now)
      extract_raw(env, "\"data\":")
    end

    # `extract` reads a QUOTED field; `data` is a bare JSON value, so it
    # ends at the next `,` or `}` instead of at the next quote. An id is
    # an Integer or a String id; either way it stops at the same place,
    # because a String id's own quotes are inside the run.
    def self.extract_raw(envelope, prefix)
      at = envelope.index(prefix)
      return "" if at.nil?
      rest = at + prefix.length
      comma = envelope.index(",", rest)
      brace = envelope.index("}", rest)
      close = comma.nil? ? brace : (brace.nil? ? comma : (comma < brace ? comma : brace))
      return "" if close.nil?
      envelope[rest, close - rest]
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
