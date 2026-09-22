# The cryptography of the `web-push` gem (3.0.2, the version campfire
# locks), ported onto spinel's openssl package: RFC 8291 payload
# encryption and the RFC 8292 VAPID header. `runtime/spinel/web_push.rb`
# sends what these build; this file opens no socket, so a compiled
# program can pin it against fixed vectors.
#
# SPINEL ONLY. The ruby family has the gem itself, and the strict
# targets have no openssl to write it over; neither loads this file.
#
# BYTE-FOR-BYTE THE GEM, NOT THE RFC'S EXAMPLE. The gem pads the record
# with "\x02\x00" where RFC 8291 section 5 prints "\x02", and writes the
# record size as the ciphertext's length where the RFC's example writes
# 4096. Both are valid RFC 8188 records, and the gem's are what a
# campfire running under Rails sends, so they are what this sends. The
# RFC's published values still pin everything up to the cipher: the
# ECDH secret, the three HKDF steps, the CEK and the nonce.
#
# WHERE THE GEM'S SPELLING HAD TO CHANGE. The gem builds keys through
# `OpenSSL::BN`, `EC::Group` and `EC::Point`, and signs through the `jwt`
# gem. spinel's package deliberately has none of the three: a key is its
# bytes (`from_private_bytes`, `public_key_bytes`, the X9.62 0x04 || X ||
# Y point), and `sign_raw` answers the raw `r || s` a JWS wants, so the
# DER-to-raw step the jwt gem performs is not needed at all. The JWT is
# three base64url segments and is assembled here.
require "openssl"
require "json"
require "securerandom"
require_relative "base64"

module WebPush
  # The gem's error hierarchy lives in runtime/gem_facades.rb; this one
  # error is raised before any of it is loaded by a program that uses
  # the crypto alone.
  class EncryptionError < StandardError
  end

  module Encryption
    CURVE = "prime256v1"

    # `WebPush::Encryption.encrypt(message, p256dh, auth)`, the gem's
    # signature: a fresh server key and a fresh salt per message, as
    # RFC 8291 requires. `p256dh` and `auth` are the subscription's keys
    # as the browser handed them out, base64url.
    def self.encrypt(message, p256dh, auth)
      server = OpenSSL::PKey::EC.generate(CURVE)
      encrypt_with(message, p256dh, auth, server.private_key_bytes, SecureRandom.random_bytes(16))
    end

    # The whole operation with the two random inputs supplied, so a
    # test can pin it against the gem's output for the same inputs.
    # Nothing else calls this with anything but fresh ones.
    def self.encrypt_with(message, p256dh, auth, server_private, salt)
      raise ArgumentError, "message cannot be blank" if message.empty?
      raise ArgumentError, "p256dh cannot be blank" if p256dh.empty?
      raise ArgumentError, "auth cannot be blank" if auth.empty?

      server = OpenSSL::PKey::EC.from_private_bytes(CURVE, server_private)
      client_public = Base64.urlsafe_decode64(p256dh)
      client_auth = Base64.urlsafe_decode64(auth)
      server_public = server.public_key_bytes

      shared_secret = server.dh_compute_key(client_public)

      info = "WebPush: info\0" + client_public + server_public
      prk = OpenSSL::KDF.hkdf(shared_secret, salt: client_auth, info: info, hash: "SHA256", length: 32)
      cek = OpenSSL::KDF.hkdf(prk, salt: salt, info: "Content-Encoding: aes128gcm\0", hash: "SHA256", length: 16)
      nonce = OpenSSL::KDF.hkdf(prk, salt: salt, info: "Content-Encoding: nonce\0", hash: "SHA256", length: 12)

      cipher = OpenSSL::Cipher.new("aes-128-gcm")
      cipher.encrypt
      cipher.key = cek
      cipher.iv = nonce
      cipher.auth_data = ""
      ciphertext = cipher.update(message + "\2\0") + cipher.final + cipher.auth_tag

      rs = ciphertext.bytesize
      raise ArgumentError, "encrypted payload is too big" if rs > 4096

      # The RFC 8188 header: salt(16) || rs(uint32 BE) || idlen(1) ||
      # keyid, where the keyid is the server's public point.
      header = salt + [rs].pack("N") + [server_public.bytesize].pack("C") + server_public
      header + ciphertext
    end
  end

  # RFC 8292: `Authorization: vapid t=<JWT>,k=<public key>`, the JWT
  # ES256-signed by the application server's key. campfire configures
  # the key pair as the gem's `VapidKey` prints it — base64url of the
  # 65-byte public point and of the 32-byte private scalar.
  module Vapid
    CURVE = "prime256v1"

    # `expiration` is seconds from now; the gem's default is twelve
    # hours, and a push service rejects a claim more than 24 away.
    def self.header(audience, subject, public_key, private_key, expiration)
      key = OpenSSL::PKey::EC.from_private_bytes(CURVE, Base64.urlsafe_decode64(private_key))
      "vapid t=#{jwt(audience, subject, key, Time.now.to_i + expiration)},k=#{Base64.urlsafe_encode64_nopad(key.public_key_bytes)}"
    end

    # The same header and claim order the jwt gem writes for the gem's
    # `JWT.encode(payload, key, "ES256", { typ:, alg: })`.
    def self.jwt(audience, subject, key, exp)
      head = Base64.urlsafe_encode64_nopad("{\"typ\":\"JWT\",\"alg\":\"ES256\"}")
      claims = Base64.urlsafe_encode64_nopad("{\"aud\":#{JSON.generate(audience)},\"exp\":#{exp},\"sub\":#{JSON.generate(subject)}}")
      input = head + "." + claims
      input + "." + Base64.urlsafe_encode64_nopad(key.sign_raw("SHA256", input))
    end
  end
end
