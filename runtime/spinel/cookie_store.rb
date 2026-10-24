# frozen_string_literal: true
# The session cookie, as Rails' `ActionDispatch::Session::CookieStore`
# writes it, and the flash inside it, as `ActionDispatch::Flash` keeps it.
# The two ruby-family dispatchers restore the session through `restore`
# and write it back through `cookie_value`; between the two they move the
# flash in and out of it (`inbound_flash`, `commit_flash`).
#
# Measured against Rails 8.1.4 with `load_defaults 8.1`:
#
# * THE SESSION IS ENCRYPTED, through the jar `cookies.encrypted` is
#   (ActionController::MessageEncryptor: AES-256-GCM under
#   "authenticated encrypted cookie", purpose `cookie.<session key>`).
#   A value that does not decrypt for that name reads as an empty
#   session, as Rails reads it: a tampered cookie, one encrypted under
#   another secret, one written for another cookie, and the signed
#   `k=v` cookie this runtime wrote before (users of an emitted app sign
#   in once more, as they would after Rails rejected their cookie).
# * THE PAYLOAD IS THE SESSION AS JSON (`cookies_serializer :json`), so
#   values keep their types across requests: an Integer comes back an
#   Integer, a Hash a Hash with String keys. A session minted by Rails
#   restores here, and one written here restores in Rails;
#   `cookies.encrypted[session_key]` answers the same Hash in both.
# * A WRITTEN SESSION CARRIES A `session_id` (32 hex characters), first,
#   as Rails' store gives every session it loads. Here it is added when
#   the session is written, not when it is read: before its first write
#   a session has no `session_id`, where Rails would show one.
# * THE FLASH LIVES IN THE SESSION, under "flash" as
#   `{"discard" => [], "flashes" => {...}}`, Rails' `to_session_value`.
#   Messages read from it are shown once and dropped, unless the action
#   sets them again (`ActionDispatch::Flash#to_persisted`).
#
# The dispatchers compare the session's JSON before and after the action
# and write the cookie only when it changed: the cookie itself differs on
# every write (a fresh IV). Rails rewrites the cookie on every request
# that loaded the session; the session the browser holds is the same.
#
# Ruby family only, like `request_forgery_protection.rb` beside it.
# Required from BOTH boots, after the controller runtime.
module ActionDispatch
  module CookieStore
    # The session `raw` carries for the cookie called `name`; empty when
    # the cookie is absent or does not read (see the header).
    def self.restore(raw, name)
      session = ActionDispatch::Session.new
      return session if raw == ""
      json = ActionController::MessageEncryptor.decrypted_json(
        Rails.application.secret_key_base, raw, "cookie." + name
      )
      return session if json == ""
      data = ActionController::EncryptedCookieJar.parse(json)
      # A cookie whose message is not an object (Rails' store reads it as
      # no session at all) restores empty.
      return session unless data.is_a?(Hash)
      data.each { |k, v| session[k.to_s] = v }
      session
    end

    # The session's plain encoding: its JSON, nil values dropped as Rails'
    # store drops them. What the dispatchers compare to decide whether to
    # write, and what `cookie_value` encrypts.
    def self.payload(session)
      ActiveSupport::JSON.encode(session.to_hash)
    end

    # The Set-Cookie value for `session` under `name`: its JSON, with a
    # `session_id` first when it has none yet, encrypted.
    def self.cookie_value(session, name)
      data = session.to_hash
      unless data.key?("session_id")
        data = { "session_id" => ActionDispatch::CookieStore.generate_sid }.merge(data)
      end
      ActionController::MessageEncryptor.encrypt(
        Rails.application.secret_key_base,
        ActiveSupport::JSON.encode(data),
        "cookie." + name, "null"
      )
    end

    # Rails' `generate_sid`: 16 random bytes as 32 hex characters.
    def self.generate_sid
      bytes = MessageDigest.secure_random_bytes(16)
      out = +""
      i = 0
      while i < bytes.bytesize
        out << format("%02x", bytes.getbyte(i))
        i += 1
      end
      out
    end

    # The messages the session carries in from the previous request, for
    # `ActionDispatch::Flash.new`: Rails' `FlashHash.from_session_value`,
    # the "flashes" less the "discard" list. A message the flash cannot
    # hold (neither `notice` nor `alert`, or not a String) is not one any
    # code in the app can read, and is dropped with the rest at the end.
    def self.inbound_flash(session)
      out = {}
      value = session["flash"]
      return out unless value.is_a?(Hash)
      flashes = value["flashes"]
      return out unless flashes.is_a?(Hash)
      discard = value["discard"]
      flashes.each do |k, v|
        key = k.to_s
        next unless key == "notice" || key == "alert"
        next unless v.is_a?(String)
        next if discard.is_a?(Array) && discard.include?(key)
        # to_s: Spinel keeps the boxed value boxed past is_a?, and Flash.new takes a String Hash
        out[key] = v.to_s
      end
      out
    end

    # Rails' `Request#commit_flash`: the messages set this request go into
    # the session for the next one, and a flash with nothing to keep
    # leaves the session.
    def self.commit_flash(session, flash)
      kept = flash.to_persisted
      if kept.empty?
        session.delete("flash")
      else
        session["flash"] = { "discard" => [], "flashes" => kept }
      end
      session
    end
  end
end
