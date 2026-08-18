# ActiveRecord::SignedId — the tokens `record.signed_id(purpose:)` mints
# and `Model.find_signed(id, purpose:)` reads back.
#
# Campfire puts one in a URL on every rendered avatar
# (`route_for :user_avatar, user.avatar_token`), so this is on the path
# of any page showing a person, not a corner of the model API.
#
# The wire format is ActiveSupport's, shared with signed cookies and
# implemented once next door in action_controller/message_verifier.rb.
# Two things differ from a cookie and both are Rails' choices, not ours:
#
#   digest   HMAC-SHA256, not the cookie jar's SHA1
#   message  the id as a bare JSON Integer (`123`), not a quoted String
#
# A REOPEN-STYLE file, outside the strict-target runtime_loader tables
# for the same reason connection.rb is: PBKDF2 + HMAC live in
# MessageDigest, which only the ruby-family trees ship.
module ActiveRecord
  module SignedId
    # Rails' `ActiveRecord::Base.signed_id_verifier`'s salt. Not
    # configurable in the corpus; an app that set one would need it
    # lifted at ingest.
    SALT = "active_record/signed_id"

    # Rails combines the model name with the caller's purpose
    # (`combine_signed_id_purposes` → "user/avatar"). That join happens
    # at the CALL SITE in src/lower/signed_id.rs, where the model name
    # is a compile-time fact — what arrives here is already combined,
    # which keeps this file free of any `self.class.name` reflection.
    #
    # `expires_in` is SECONDS, with 0 meaning "never" — Rails' default
    # for `signed_id` and what every unexpiring caller passes.
    def self.generate(id, purpose, expires_in)
      exp = "null"
      if expires_in > 0
        exp = "\"" +
              ActionController::MessageVerifier.iso8601_ms(Time.now + expires_in) +
              "\""
      end
      ActionController::MessageVerifier.envelope(
        Rails.application.secret_key_base, SALT, id.to_s, purpose, exp, false
      )
    end

    # The record id `token` carries, or 0 when it does not verify — a
    # tampered token, one signed for a different purpose, and an expired
    # one are all the same answer, which is what Rails does too (it
    # raises one InvalidSignature for all three).
    #
    # DIVERGENCE, recorded in docs/pipeline/runtime.md: 0 is a sentinel,
    # so the lowered `find_signed!` raises RecordNotFound where Rails
    # raises ActiveSupport::MessageVerifier::InvalidSignature. Both are
    # a 404 through the dispatcher; a controller that rescues the
    # signature error BY NAME (campfire's Users::AvatarsController does)
    # would not catch this one.
    def self.verified_id(token, purpose)
      json = ActionController::MessageVerifier.verified_json(
        Rails.application.secret_key_base, SALT, token, purpose, false
      )
      return 0 if json == ""
      json.to_i
    end
  end
end
