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
# comment in message_verifier.rb for the full table.
require "openssl"

module MessageDigest
  # HMAC-SHA1(key, msg) as 40-char lowercase hex.
  def self.hmac_sha1_hex(key, msg)
    OpenSSL::HMAC.hexdigest("SHA1", key, msg)
  end

  # HMAC-SHA256(key, msg) as 64-char lowercase hex.
  def self.hmac_sha256_hex(key, msg)
    OpenSSL::HMAC.hexdigest("SHA256", key, msg)
  end

  # PBKDF2-HMAC-SHA256 as RAW BYTES (not hex, not base64): the derived
  # key is the HMAC key above, and HMAC keys are bytes.
  def self.pbkdf2_sha256(secret, salt, iters, dklen)
    OpenSSL::PKCS5.pbkdf2_hmac(secret, salt, iters, dklen, OpenSSL::Digest::SHA256.new)
  end
end
