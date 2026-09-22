# Driver for tests/spinel_web_push_crypto.rs — COMPILED BY SPINEL, not
# run under CRuby: runtime/spinel/web_push_crypto.rb is written against
# spinel's openssl package (keys as bytes, `sign_raw`), which CRuby's
# openssl does not spell. The harness copies it beside this file.
#
# THE ORACLE IS THE GEM, not the RFC's example body. web-push 3.0.2 pads
# with "\x02\x00" and writes rs as the ciphertext length, so its body
# differs from RFC 8291 section 5 after the key schedule; GEM_BODY is
# what the gem produced for the RFC's inputs, with `EC.generate` and
# `Random#bytes` pinned to the RFC's server key and salt:
#
#   gem "web-push", "3.0.2"; require "web-push"
#   key = <an OpenSSL::PKey::EC built from AS_PRIV>
#   OpenSSL::PKey::EC.singleton_class.prepend(Module.new { def generate(*) = key })
#   Random.prepend(Module.new { def bytes(n) = SALT })
#   puts WebPush::Encryption.encrypt(PLAINTEXT, UA_PUB, AUTH).unpack1("H*")
#
# The randomized paths — a fresh key per message, a randomized ECDSA
# signature — cannot be pinned to bytes, so they are checked by their
# inverses: the receiver's side of RFC 8291 decrypts what was sent, and
# the signature verifies (and a tampered one does not).
require_relative "web_push_crypto"

def hx(s) = [s].pack("H*")

def check(name, ok)
  puts "#{ok ? "ok" : "FAIL"} #{name}"
end

PLAINTEXT = "When I grow up, I want to be a watermelon"
AS_PRIV = hx("c9f58f89813e9f8e872e71f42aa64e1757c9254dcc62b72ddc010bb4043ea11c")
SALT = hx("0c6bfaadad67958803092d454676f397")
UA_PUB = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
UA_PRIV = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94"
AUTH = "BTBZMqHH6r4Tts7J_aSIgg"
GEM_BODY = "0c6bfaadad67958803092d454676f3970000003b4104fe33f4ab0dea71914db55823f73b54948f41306d920732dbb9a59a53286482200e597a7b7bc260ba1c227998580992e93973002f3012a28ae8f06bbb78e5ec0ff297de5b429bba7153d3a4ae0caa091fd425f3b4b5414add8ab37a19c1bbb05cf5cb5b2a2e0562d5586392655400275f8719c44a2fc0cd4aba52dd"

body = WebPush::Encryption.encrypt_with(PLAINTEXT, UA_PUB, AUTH, AS_PRIV, SALT)
check "pinned inputs give the gem's bytes", body.unpack1("H*") == GEM_BODY

# The receiver's half of RFC 8291, over a message sent with a FRESH
# server key and salt: read the header, derive the same keys from the
# user agent's private key, and open the record.
def receive(body)
  salt = body.byteslice(0, 16)
  rs = body.byteslice(16, 4).unpack1("N")
  idlen = body.getbyte(20)
  server_public = body.byteslice(21, idlen)
  ct = body.byteslice(21 + idlen, body.bytesize - 21 - idlen)
  return "rs mismatch" if rs != ct.bytesize
  ua = OpenSSL::PKey::EC.from_private_bytes("prime256v1", Base64.urlsafe_decode64(UA_PRIV))
  secret = ua.dh_compute_key(server_public)
  info = "WebPush: info\0" + ua.public_key_bytes + server_public
  prk = OpenSSL::KDF.hkdf(secret, salt: Base64.urlsafe_decode64(AUTH), info: info, hash: "SHA256", length: 32)
  cek = OpenSSL::KDF.hkdf(prk, salt: salt, info: "Content-Encoding: aes128gcm\0", hash: "SHA256", length: 16)
  nonce = OpenSSL::KDF.hkdf(prk, salt: salt, info: "Content-Encoding: nonce\0", hash: "SHA256", length: 12)
  d = OpenSSL::Cipher.new("aes-128-gcm")
  d.decrypt
  d.key = cek
  d.iv = nonce
  d.auth_data = ""
  d.auth_tag = ct.byteslice(ct.bytesize - 16, 16)
  d.update(ct.byteslice(0, ct.bytesize - 16)) + d.final
end

message = "{\"title\":\"héllo\"}"
fresh = WebPush::Encryption.encrypt(message, UA_PUB, AUTH)
check "a fresh message opens on the receiver's side", receive(fresh) == (message + "\2\0").b
again = WebPush::Encryption.encrypt(message, UA_PUB, AUTH)
check "each message gets its own salt", fresh.byteslice(0, 16) != again.byteslice(0, 16)

refused = begin
  WebPush::Encryption.encrypt("", UA_PUB, AUTH)
  false
rescue ArgumentError
  true
end
check "an empty message is refused, as the gem refuses it", refused

VAPID_PUB = "BLNdWxzIxEQa54YJimsS5usDSGIN3N_4zjtnhsNV41cMQ4tGQ4QigFUnBOfNCe6eYiLJsEWOGqA0wsYc9K0Eo9k="
VAPID_PRIV = "Vf5AhgC2rPgvD6mnBtKnhOAfkAjDpCboR_zo91kmDk4="
header = WebPush::Vapid.header("https://fcm.googleapis.com", "mailto:support@37signals.com", VAPID_PUB, VAPID_PRIV, 43200)
jwt = header.byteslice(8, header.index(",k=").to_i - 8)
k = header.byteslice(header.index(",k=").to_i + 3, header.bytesize)
check "k= is the unpadded public key", k + "=" == VAPID_PUB
parts = jwt.split(".")
check "the JWT header is the jwt gem's", Base64.urlsafe_decode64(parts[0]) == "{\"typ\":\"JWT\",\"alg\":\"ES256\"}"
claims = Base64.urlsafe_decode64(parts[1])
check "the claims name the audience and subject", claims.include?("\"aud\":\"https://fcm.googleapis.com\"") && claims.include?("\"sub\":\"mailto:support@37signals.com\"")
verifier = OpenSSL::PKey::EC.from_public_bytes("prime256v1", Base64.urlsafe_decode64(VAPID_PUB))
sig = Base64.urlsafe_decode64(parts[2])
check "the signature is raw r||s", sig.bytesize == 64
check "the signature verifies", verifier.verify_raw("SHA256", sig, parts[0] + "." + parts[1])
check "a tampered claim does not", !verifier.verify_raw("SHA256", sig, parts[0] + "." + parts[1] + "x")
puts "done"
