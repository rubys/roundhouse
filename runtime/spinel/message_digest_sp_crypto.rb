# Keyed-digest primitives for the spinel binary over sp_crypto — NOT
# YET THE SHIPPED IMPLEMENTATION. Both entry points it calls are on
# matz/spinel#3770 and not in spinel master, so linking this would fail
# `ld` (which is exactly how CI found out). `message_digest.rb` beside
# it ships a raising stub until that lands; when it does, delete the
# stub and rename this file over it — nothing else changes, since the
# framework Ruby above calls only the three functions below.
#
# The sp_crypto half of the two-layer split (message_digest_cruby.rb is the OpenSSL half, and
# `ruby_runtime_files` swaps it in at this path for the CRuby/JRuby
# trees). The framework Ruby above this — action_controller/
# message_verifier.rb — calls only the three functions below and so
# compiles unchanged for every ruby-family target.
#
# sp_crypto ships with spinel (lib/sp_crypto.c, always linked). Two of
# the three entry points landed for exactly this use — reading a Rails
# signed cookie — see matz/spinel#3769 and matz/spinel#3770:
#
#   * `_len` on PBKDF2, because Rails derives at dkLen 64 and the one-
#     block helper topped out at 32.
#   * HMAC-SHA1 at all, because that is the digest Rails signs cookies
#     with, and SHA-1 was previously exposed only as a whole-protocol
#     helper (the WebSocket handshake).
#
# Static-buffer contract: every sp_crypto return points at a per-function
# static that the next call to the same function clobbers, so each result
# is copied (`+ ""`) before it can outlive the next call.
module SpCrypto
  ffi_func :sp_crypto_hmac_sha1_hex,            [:str, :str],             :str
  ffi_func :sp_crypto_hmac_sha256_hex,          [:str, :str],             :str
  ffi_func :sp_crypto_pbkdf2_sha256_b64url_len, [:str, :str, :int, :int], :str
  ffi_func :sp_crypto_b64url_decode,            [:str],                   :str
end

module MessageDigest
  def self.hmac_sha1_hex(key, msg)
    SpCrypto.sp_crypto_hmac_sha1_hex(key, msg) + ""
  end

  def self.hmac_sha256_hex(key, msg)
    SpCrypto.sp_crypto_hmac_sha256_hex(key, msg) + ""
  end

  # sp_crypto returns the derived key base64url-encoded; the callers want
  # the raw bytes (an HMAC key is bytes), so decode on the way out. Both
  # results copy off the static buffer before the next FFI call.
  def self.pbkdf2_sha256(secret, salt, iters, dklen)
    b64 = SpCrypto.sp_crypto_pbkdf2_sha256_b64url_len(secret, salt, iters, dklen) + ""
    SpCrypto.sp_crypto_b64url_decode(b64) + ""
  end
end
