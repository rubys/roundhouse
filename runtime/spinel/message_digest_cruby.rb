# Keyed-digest primitives for the CRuby/JRuby trees — the OpenSSL half
# of the two-layer split. `runtime/message_digest.rb` in the spinel tree
# is the same surface over sp_crypto's FFI; `ruby_runtime_files` swaps
# this file in at that path (same contract as db_cruby.rb → db.rb), so
# the framework Ruby above it (action_controller/message_verifier.rb)
# is written once and compiles for every ruby-family target.
#
# The surface is exactly what reproducing Rails' signed messages needs,
# and no more:
#
#   * HMAC-SHA1  — the digest Rails signs COOKIES with
#                  (`signed_cookie_digest || "SHA1"`; no `load_defaults`
#                  version sets `cookies_digest`, 8.2 included).
#   * HMAC-SHA256 — the digest Rails signs IDs with (`signed_id` /
#                  `find_signed!` are generated with digest: "SHA256").
#   * PBKDF2-HMAC-SHA256 — key derivation, SHA-256 since
#                  `load_defaults 7.0` set key_generator_hash_digest_class.
#
# Both digests are needed because Rails genuinely uses both; see the
# comment in message_verifier.rb for the full table. CSRF adds raw
# HMAC-SHA256, `secure_random_bytes`, and constant-time compare.
require "openssl"
require "securerandom"

module MessageDigest
  # HMAC-SHA1(key, msg) as 40-char lowercase hex.
  def self.hmac_sha1_hex(key, msg)
    keyed_hmac("SHA1", key, msg).hexdigest
  end

  # HMAC-SHA256(key, msg) as 64-char lowercase hex.
  def self.hmac_sha256_hex(key, msg)
    keyed_hmac("SHA256", key, msg).hexdigest
  end

  # Raw HMAC-SHA256 bytes (CSRF global token, not the hex cookies use).
  def self.hmac_sha256(key, msg)
    keyed_hmac("SHA256", key, msg).digest
  end

  # An HMAC over `msg`, from a per-thread instance already keyed with
  # `key`. The keys are the app's derived secrets (a handful per
  # process), and `OpenSSL::HMAC.hexdigest(digest, key, msg)` set up the
  # digest and the key schedule on every call: verifying the session
  # cookie on every request made HMAC#initialize 4-6% of campfire's small
  # routes. `reset` returns the instance to its keyed state. Per thread
  # because an HMAC context is mutable; capped because keys could in
  # principle vary without bound.
  KEYED_HMAC_CAP = 64

  def self.keyed_hmac(digest, key, msg)
    cache = (Thread.current[:rh_keyed_hmac] ||= {})
    ck = digest + "\0" + key
    h = cache[ck]
    if h.nil?
      cache.clear if cache.size >= KEYED_HMAC_CAP
      h = cache[ck] = OpenSSL::HMAC.new(key, digest)
    else
      h.reset
    end
    h.update(msg)
  end

  def self.secure_random_bytes(n)
    SecureRandom.random_bytes(n)
  end

  def self.secure_compare(a, b)
    return false if a.bytesize != b.bytesize
    diff = 0
    i = 0
    while i < a.bytesize
      diff = diff | (a.getbyte(i) ^ b.getbyte(i))
      i += 1
    end
    diff == 0
  end

  # PBKDF2-HMAC-SHA256 as RAW BYTES (not hex, not base64): the derived
  # key is the HMAC key above, and HMAC keys are bytes.
  def self.pbkdf2_sha256(secret, salt, iters, dklen)
    OpenSSL::PKCS5.pbkdf2_hmac(secret, salt, iters, dklen, OpenSSL::Digest::SHA256.new)
  end
end
