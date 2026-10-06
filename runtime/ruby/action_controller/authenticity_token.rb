# Rails-style masked authenticity tokens: one session secret, a
# per-render one-time pad, XOR + URL-safe Base64. Turbo's
# `csrf-token` meta and `X-CSRF-Token` header consume the masked
# value unchanged.
#
# Write-once for every ruby-family target. Strict-target `base.rb`
# still answers an empty token (no `Current.session`); this file is
# required from `action_controller.rb`, off those tables, like
# `current.rb`. Masking is byte XOR — transpilable — and the only
# primitives below are `MessageDigest.secure_random_bytes`,
# `hmac_sha256`, and `secure_compare`.
module ActionController
  module AuthenticityToken
    LENGTH = 32
    GLOBAL_SCOPE = "!real_csrf_token"

    def self.masked
      raw_b64 = ensure_session_token
      return "" if raw_b64.empty?
      raw = decode(raw_b64)
      return "" if raw.length != LENGTH
      pad = MessageDigest.secure_random_bytes(LENGTH)
      return "" if pad.length != LENGTH
      Base64.urlsafe_encode64_nopad(pad + xor(pad, raw))
    end

    def self.valid?(given, expected_b64)
      return false if expected_b64.empty?
      return false if given.empty?
      expected = decode(expected_b64)
      return false if expected.length != LENGTH
      actual = unmask(given)
      return false if actual.length != LENGTH
      return true if MessageDigest.secure_compare(actual, expected)
      MessageDigest.secure_compare(actual, hmac_global(expected))
    end

    def self.ensure_session_token
      session = Current.session
      return "" if session.nil?
      token = session[:_csrf_token]
      if token.nil?
        token = mint
        session[:_csrf_token] = token
      end
      token.to_s
    end

    def self.mint
      Base64.urlsafe_encode64_nopad(MessageDigest.secure_random_bytes(LENGTH))
    end

    def self.unmask(encoded)
      decoded = decode(encoded)
      n = decoded.length
      return decoded if n == LENGTH
      return "" if n != LENGTH + LENGTH
      xor(decoded[0, LENGTH].to_s, decoded[LENGTH, LENGTH].to_s)
    end

    def self.decode(encoded)
      Base64.urlsafe_decode64(encoded)
    end

    def self.xor(a, b)
      out = ""
      i = 0
      while i < LENGTH
        out = out + (a.getbyte(i) ^ b.getbyte(i)).chr
        i += 1
      end
      out
    end

    def self.hmac_global(raw)
      MessageDigest.hmac_sha256(raw, GLOBAL_SCOPE)
    end
  end

  def self.masked_authenticity_token
    AuthenticityToken.masked
  end

  def self.csrf_token_valid?(given, expected)
    AuthenticityToken.valid?(given, expected)
  end
end
