# Keyed-digest primitives for the spinel binary — the stub half of the
# two-layer split (message_digest_cruby.rb is the OpenSSL half, which
# `ruby_runtime_files`/`jruby_runtime_files` swap in at this path).
#
# The real implementation is `message_digest_sp_crypto.rb` beside this
# file, and it is not wired up yet: it calls
# `sp_crypto_hmac_sha1_hex` and `sp_crypto_pbkdf2_sha256_b64url_len`,
# both of which are on matz/spinel#3770 and not in spinel master. A
# binary declaring FFI entry points the linked sp_crypto doesn't export
# fails at `ld`, so shipping it would take the whole spinel target down
# — signed cookies or not.
#
# So the spinel binary raises here rather than silently mis-signing.
# Nothing in the blog fixture reaches these (it signs nothing), which is
# why the target still builds and serves; an app that does reach them —
# campfire's session cookie — gets a loud, named failure instead of a
# cookie no Rails would accept.
#
# WHEN #3770 LANDS: delete this file and rename
# message_digest_sp_crypto.rb over it. The framework Ruby above calls
# only these three functions, so nothing else changes.
# Each body ends in a `""` the raise makes unreachable: a method that
# only raises types as `void`, and the verifier above needs these to
# carry String (the shared-runtime rule that non-void methods END in a
# read — spinel's C emit otherwise returns void into a `const char *`).
module MessageDigest
  UNAVAILABLE = "keyed digests need matz/spinel#3770 (sp_crypto_hmac_sha1_hex " \
                "+ sp_crypto_pbkdf2_sha256_b64url_len); see " \
                "runtime/message_digest_sp_crypto.rb"

  def self.hmac_sha1_hex(key, msg)
    raise UNAVAILABLE
    ""
  end

  def self.hmac_sha256_hex(key, msg)
    raise UNAVAILABLE
    ""
  end

  def self.pbkdf2_sha256(secret, salt, iters, dklen)
    raise UNAVAILABLE
    ""
  end
end
