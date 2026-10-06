# Keyed-digest primitives for the spinel binary — the sp_crypto half of
# the two-layer split (message_digest_cruby.rb is the OpenSSL half, and
# `ruby_runtime_files` swaps it in at this path for the CRuby/JRuby
# trees). The framework Ruby above this — action_controller/
# message_verifier.rb — calls only the three functions below and so
# compiles unchanged for every ruby-family target.
#
# sp_crypto ships with spinel (lib/sp_crypto.c, always linked). Two of
# the three entry points landed for exactly this use — reading a Rails
# signed cookie — in matz/spinel#3770 (merged 2026-08-10, spinel
# 6fa33d8c), filed as matz/spinel#3769:
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
  ffi_func :sp_crypto_random_b64url,            [:int],                   :str
end

module MessageDigest
  def self.hmac_sha1_hex(key, msg)
    SpCrypto.sp_crypto_hmac_sha1_hex(key, msg) + ""
  end

  def self.hmac_sha256_hex(key, msg)
    SpCrypto.sp_crypto_hmac_sha256_hex(key, msg) + ""
  end

  def self.hmac_sha256(key, msg)
    hex = hmac_sha256_hex(key, msg)
    out = ""
    i = 0
    n = hex.length
    while i + 2 <= n
      out = out + hex[i, 2].to_s.to_i(16).chr
      i += 2
    end
    out
  end

  def self.secure_random_bytes(n)
    Base64.urlsafe_decode64(SpCrypto.sp_crypto_random_b64url(n) + "")
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

  # sp_crypto returns the derived key base64url-encoded; the callers want
  # the raw bytes (an HMAC key is bytes), so decode on the way out — in
  # Ruby, NOT through `sp_crypto_b64url_decode`. That one hands back a
  # C string with no length, so a key with a zero byte in it arrived cut
  # at the zero: about one SECRET_KEY_BASE in five derives such a key, and
  # on those deployments every signed cookie, signed id, sgid and blob URL
  # the binary made was signed with a key a few bytes long — forgeable,
  # and matching nothing Rails signs. (HMAC itself is binary-safe: it
  # reads the key's length off the String, `sp_str_byte_len`.) Found by
  # running Rails' vectors on the binary; pinned by the `nul_key` cases in
  # tests/rails_compat_vectors_spinel.rs, secrets whose keys hold a zero.
  # The FFI declaration is gone with it, so nothing reaches for it again.
  def self.pbkdf2_sha256(secret, salt, iters, dklen)
    b64 = SpCrypto.sp_crypto_pbkdf2_sha256_b64url_len(secret, salt, iters, dklen) + ""
    Base64.urlsafe_decode64(b64)
  end
end
