# `Turbo::Streams::StreamName` — the SIGNING half of Turbo's stream
# names, for a channel that guards its own stream.
#
# Turbo puts the stream name in the page, inside
# `<turbo-cable-stream-source signed-stream-name="…">`, and the client
# hands it straight back on subscribe. Rails HMAC-signs it on the way
# out and verifies on the way in, so a name the user edited is refused
# before it reaches a channel.
#
# THIS RUNTIME DOES NOT SIGN, and says so at both ends: the value
# `ActionView::ViewHelpers.turbo_stream_from` writes is
# `<base64-of-JSON>--unsigned`, and `Cable::Connection#decode_stream_name`
# reads it back by splitting on `--` and ignoring the suffix. This file
# is the third end of the same wire and matches them rather than
# inventing a fourth spelling.
#
# WHAT THAT DOES AND DOES NOT COST. An unsigned name is tamperable: a
# client can subscribe to any stream it can spell. That is a real
# divergence and it is ledgered in docs/pipeline/runtime.md. What it
# does NOT do is defeat campfire's `RoomMessagesChannel`, which is the
# one channel in the corpus that guards its stream — the name is only
# the channel's INPUT. It re-derives the room from the name and then
# asks `user.rooms.find_by(id: room.id)`, so a forged name buys a
# subscription to a room the user is already a member of. Real HMAC
# signing belongs here, with `turbo_stream_from` and `decode_stream_name`
# changed in the same commit, once the key derivation question
# (message_verifier.rb's iteration count vs Rails') is settled.
#
# ONLY WHAT IS REACHED. The module's `signed_stream_name(streamables)`
# and its `stream_name_from` helper are not here: `turbo_stream_from`
# computes the encoded name itself, and campfire's `extend
# Turbo::Streams::StreamName` — the half that would supply them — is
# dropped at ingest with a `lower_residue` entry naming it. Adding a
# generator nothing calls would be a second spelling of the encoding to
# keep in step with the other two.
require_relative "base64"

module Turbo
  module Streams
    module StreamName
      # The inverse of `turbo_stream_from`'s encoding. `nil` for
      # anything that is not one of our names, which is what a caller
      # checking `if stream_name = verified_stream_name_from_params`
      # expects — campfire's channel rejects the subscription on nil.
      def self.verified(signed)
        return nil if signed.nil?

        encoded = signed.to_s.split("--", 2)[0]
        return nil if encoded.nil? || encoded.empty?

        begin
          JSON.parse(Base64.strict_decode64(encoded))
        rescue ArgumentError, JSON::ParserError
          nil
        end
      end

      # Mixed into a channel with `include Turbo::Streams::StreamName
      # ::ClassMethods` (the spelling turbo-rails uses, and the name is
      # its historical accident — these are INSTANCE methods on the
      # channel).
      #
      # Rails routes this through `self.class.verified_stream_name` so a
      # channel can override verification; nothing in the corpus does,
      # and the indirection needs the `extend` half of the module, which
      # ingest drops. Straight to the module function instead.
      #
      # `params` is `ActionCable::Channel::Base#params`, which RAISES —
      # channel subscription dispatch is not implemented, so no
      # subscription frame is ever bound (see runtime/action_cable.rb).
      # That raise is the honest answer here too: a `subscribed` that
      # returned a plausible stream name would look like it worked.
      module ClassMethods
        def verified_stream_name_from_params
          StreamName.verified(params[:signed_stream_name])
        end
      end
    end
  end
end
