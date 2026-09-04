# ActiveRecord::SignedId — the tokens `record.signed_id(purpose:)` mints
# and `Model.find_signed(id, purpose:)` reads back.
#
# Campfire puts one in a URL on every rendered avatar
# (`route_for :user_avatar, user.avatar_token`), so this is on the path
# of any page showing a person, not a corner of the model API.
#
# The wire format is ActiveSupport's, implemented next door in
# action_controller/message_verifier.rb. Three things differ from a
# signed cookie and all three are Rails' choices, not ours:
#
#   digest    HMAC-SHA256, not the cookie jar's SHA1
#   envelope  `{"_rails":{"data":123,"pur":…}}` — the id verbatim as
#             JSON, not base64 in a `message` field
#   exp       OMITTED when there is none, where a cookie always carries
#             `"exp":null`
#
# All three measured against campfire under Rails 8.2 by minting
# `user.signed_id(purpose: :avatar)` and reading the bytes. The file used
# to build the cookie envelope for a signed id: same secret, same salt,
# same digest, different shape — so an `avatar_token` minted here was
# rejected by Rails and vice versa, and no test saw it because both sides
# of every test were this file.
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
      # "" means "no exp key at all", which is what Rails writes for an
      # unexpiring signed id — not `"exp":null`, the cookie jar's shape.
      exp = ""
      if expires_in > 0
        exp = "\"" +
              ActionController::MessageVerifier.iso8601_ms(Time.now + expires_in) +
              "\""
      end
      ActionController::MessageVerifier.data_envelope(
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
      json = ActionController::MessageVerifier.verified_data_json(
        Rails.application.secret_key_base, SALT, token, purpose, false
      )
      return 0 if json == ""
      json.to_i
    end

    # The BANG form's half: Rails' `find_signed!` raises
    # `ActiveSupport::MessageVerifier::InvalidSignature` for a token that
    # does not verify, and `RecordNotFound` only when a token that DOES
    # verify names no row. The sentinel above cannot tell those apart —
    # `find(0)` reports both as "Couldn't find … with id=0" — so the two
    # readings are two methods rather than one plus a guess.
    #
    # It matters because the name is what a rescue matches:
    # campfire's `Users::AvatarsController` rescues the signature error
    # BY NAME over an avatar URL carrying a signed id, and against a
    # RecordNotFound that rescue never fired.
    # Over `verified_id` rather than beside it: the sentinel already
    # marks the failure, and a second copy of the verify call would be
    # eight more untyped sites in the shared corpus for a fact one
    # comparison already carries. A real row id is never 0.
    def self.verified_id!(token, purpose)
      id = ActiveRecord::SignedId.verified_id(token, purpose)
      raise ActiveSupport::MessageVerifier::InvalidSignature if id == 0
      id
    end
  end
end
