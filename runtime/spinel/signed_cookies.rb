# frozen_string_literal: true
# The cookies the two ruby-family dispatchers write for themselves — the
# session and the flash messages — signed, so a client can read them but
# not forge them. A value that does not verify (edited, plaintext, signed
# under another secret, or minted for another cookie name) reads as
# absent: an empty session, no flash message. Rails does the same with a
# cookie it cannot authenticate.
#
# The signature is the one `cookies.signed` already writes
# (`ActionController::MessageVerifier`: PBKDF2 key from
# SECRET_KEY_BASE, HMAC-SHA1, the `_rails` envelope with purpose
# `cookie.<name>`), so there is one signing path to trust rather than
# several. The purpose names the cookie, so a signed notice cannot be
# replayed as the session or the other way round.
#
# NOT Rails' cookies, in three ways, all stated:
#
# * SIGNED, NOT ENCRYPTED. Rails' cookie store encrypts (AES-256-GCM,
#   the "authenticated encrypted cookie" salt), so a client cannot read
#   its own session either. Here it can: the session payload is base64
#   of the url-encoded `k=v` pairs, the flash payload the message text.
#   Integrity is HMAC; confidentiality is a named residual until an
#   AES-GCM CookieStore lands in this runtime. Dispatch now emits
#   `Secure` on HTTPS and `SameSite=Lax` on the session/flash cookies.
# * NOT INTEROPERABLE. The session payload is this runtime's `k=v`
#   encoding, not Rails' JSON, so a Rails session cookie does not
#   restore here (it reads as empty) and ours would not restore in
#   Rails. A migration from Rails signs every user out of the SESSION —
#   campfire's login rides its own `cookies.signed[:session_token]`,
#   which does carry over.
# * THE FLASH HAS COOKIES OF ITS OWN. Rails keeps the flash inside the
#   session; here each message rides `flash_notice` / `flash_alert`.
#
# Ruby family only, like `request_forgery_protection.rb` beside it: these
# cookies are written only by the two ruby-family dispatchers. Required
# from BOTH boots, after the controller runtime.
module ActionDispatch
  module SignedCookie
    # `value` signed for the cookie called `name`.
    def self.sign(value, name)
      ActionController::MessageVerifier.generate(
        Rails.application.secret_key_base,
        ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
        value, "cookie." + name, true
      )
    end

    # The value `raw` carries when it verifies for `name`, else "" — the
    # same answer as an absent cookie. UTF-8, as the verifier answers every
    # message (the flash's `notice: "✓"` is why that matters).
    def self.verified(raw, name)
      return "" if raw == ""
      ActionController::MessageVerifier.verified(
        Rails.application.secret_key_base,
        ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
        raw, "cookie." + name, true
      )
    end
  end

  class Session
    def self.from_signed_cookie(raw, name)
      Session.from_cookie(SignedCookie.verified(raw, name))
    end

    # `plain` is `Session#to_cookie`'s encoding. The dispatchers compare
    # plain encodings to decide whether the session changed, and sign
    # only what they write.
    def self.signed_cookie(plain, name)
      SignedCookie.sign(plain, name)
    end
  end
end
