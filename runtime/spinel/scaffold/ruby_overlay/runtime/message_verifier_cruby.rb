# frozen_string_literal: true

# Memoized signed-value verification for the CRuby/JRuby trees.
#
# A browser sends the same signed cookies (the session token, the session
# itself) on every request, and `MessageVerifier.verified_json` re-derives
# the same answer each time: an HMAC over the payload, a base64 decode and
# a scan of the envelope. The answer depends only on its arguments, except
# for an expiry, so it is kept per thread, keyed by the arguments, with the
# envelope's `exp` re-checked against the clock on every hit. Rejections
# ("") are not kept: they are rare, and keeping them would let a flood of
# forged values fill the cache.
#
# Shared runtime/ruby stays as it is for the strict targets and Spinel.
module ActionController
  module MessageVerifier
    VERIFIED_CACHE_CAP = 1024

    class << self
      alias_method :verified_json_uncached, :verified_json

      def verified_json(secret, salt, signed, purpose, sha1)
        cache = (Thread.current[:rh_verified_json] ||= {})
        key = [signed, salt, purpose, sha1, secret]
        hit = cache[key]
        if !hit.nil?
          exp = hit[1]
          return hit[0] if exp == "" || exp > iso8601_ms(Time.now)
          cache.delete(key)
          return ""
        end
        json = verified_json_uncached(secret, salt, signed, purpose, sha1)
        return json if json == ""
        sep = signed.index("--")
        exp = extract(Base64.strict_decode64(signed[0, sep]), "\"exp\":\"")
        cache.clear if cache.size >= VERIFIED_CACHE_CAP
        cache[key] = [json.freeze, exp.freeze].freeze
        json
      end

      # The decoded message, from the JSON verified_json just answered:
      # json_value builds the String a character at a time (Integer#chr
      # per byte, ~30% of a signed-in request's allocations). A copy is
      # returned, as the shared method returns a fresh String.
      alias_method :verified_uncached, :verified

      def verified(secret, salt, signed, purpose, sha1)
        json = verified_json(secret, salt, signed, purpose, sha1)
        return "" if json == ""
        cache = (Thread.current[:rh_verified_value] ||= {})
        hit = cache[json]
        return hit.dup unless hit.nil?
        value = verified_uncached(secret, salt, signed, purpose, sha1)
        cache.clear if cache.size >= VERIFIED_CACHE_CAP
        cache[json] = value.dup.freeze
        value
      end

      # Signing is deterministic here: `generate` always writes an
      # unexpiring envelope (`"exp":null`), so the same arguments sign to
      # the same String. A browser's unchanged session is re-signed on
      # every response.
      alias_method :generate_uncached, :generate

      def generate(secret, salt, value, purpose, sha1)
        cache = (Thread.current[:rh_generated] ||= {})
        key = [value, salt, purpose, sha1, secret]
        hit = cache[key]
        return hit.dup unless hit.nil?
        signed = generate_uncached(secret, salt, value, purpose, sha1)
        cache.clear if cache.size >= VERIFIED_CACHE_CAP
        cache[key] = signed.dup.freeze
        signed
      end

      # Signed ids (an avatar token, a blob key) are verified the same way
      # as signed cookies: kept per thread, exp re-checked on every hit,
      # rejections never kept.
      alias_method :verified_data_json_uncached, :verified_data_json

      def verified_data_json(secret, salt, signed, purpose, sha1)
        cache = (Thread.current[:rh_verified_data_json] ||= {})
        key = [signed, salt, purpose, sha1, secret]
        hit = cache[key]
        if !hit.nil?
          exp = hit[1]
          return hit[0] if exp == "" || exp > iso8601_ms(Time.now)
          cache.delete(key)
          return ""
        end
        json = verified_data_json_uncached(secret, salt, signed, purpose, sha1)
        return json if json == ""
        sep = signed.index("--")
        exp = extract(Base64.urlsafe_decode64(signed[0, sep]), "\"exp\":\"")
        cache.clear if cache.size >= VERIFIED_CACHE_CAP
        cache[key] = [json.freeze, exp.freeze].freeze
        json
      end
    end
  end
end

# Rack::ETag's body digest through OpenSSL, which takes the CPU's SHA
# instructions where `Digest::SHA256` does not: the same hex, 1.65 ms ->
# 0.23 ms for a 396 KB campfire room page on the bench box (29% of a
# render whose message fragments were cached).
#
# One digest object per THREAD, reset by `hexdigest(body)` itself, rather
# than one per call: building it is half the cost of digesting a small
# body (`/up`'s: 740 -> 400 ns). A thread variable, not `Thread.current[]`,
# which is per fiber — under Falcon, per request.
require "openssl"

module ActionController
  class Base
    def self.body_sha256_hex(body)
      thread = Thread.current
      digest = thread.thread_variable_get(:roundhouse_sha256) ||
        thread.thread_variable_set(:roundhouse_sha256, OpenSSL::Digest.new("SHA256"))
      digest.hexdigest(body)
    end
  end
end
