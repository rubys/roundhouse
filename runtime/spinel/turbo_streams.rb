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
# `<base64-of-JSON>--unsigned`, and `StreamName.verified` below reads it
# back by splitting on `--` and ignoring the suffix. Both ends of the
# wire are in this file now: a subscribe reaches `verified` through the
# channel it named, so the decoder Cable used to keep of its own is
# gone and there is one spelling instead of two.
#
# WHAT THAT COSTS. An unsigned name is tamperable: a client can
# subscribe to any stream it can spell. Real HMAC signing belongs here,
# with `turbo_stream_from` and `verified` changed in the same commit, once the key derivation question (message_verifier.rb's
# iteration count vs Rails') is settled.
#
# AUTHORIZATION IS A SEPARATE QUESTION, and it is answered below rather
# than here: signing decides whether the name was TAMPERED WITH,
# authorization decides whether the named stream MAY BE JOINED. The
# second half is `Turbo::StreamsChannel`, which is the stock door
# campfire's `RoomStreamsAreAuthorized` is prepended onto to nail shut
# ("the stock channel as a way around it: same signed stream name, no
# membership check"). Dispatching a subscribe frame to it by name is
# what puts that guard in the path — rubys/roundhouse#71 item 4.
#
# The signing gap is NOT closed by any of that. An unsigned name is
# still tamperable, and the guard only refuses the names an app thought
# to guard; ledgered on its own in docs/pipeline/runtime.md.
#
# ONLY WHAT IS REACHED. The module's `signed_stream_name(streamables)`
# and its `stream_name_from` helper are not here: `turbo_stream_from`
# computes the encoded name itself, and campfire's `extend
# Turbo::Streams::StreamName` — the half that would supply them — is
# dropped at ingest with a `lower_residue` entry naming it. Adding a
# generator nothing calls would be a second spelling of the encoding to
# keep in step with the other two.
require_relative "base64"
# `Turbo::StreamsChannel` below subclasses `ActionCable::Channel::Base`,
# and a superclass is needed at class-definition time.
require_relative "action_cable"
# ...and its class-level broadcast API calls `Broadcasts.record`.
require_relative "broadcasts"

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
      # `params` is `ActionCable::Channel::Base#params` — the subscribe
      # frame's own identifier, bound when the frame was dispatched to
      # this channel by name. `signed_stream_name` is the attribute
      # `turbo_stream_from` wrote into the page, handed straight back.
      module ClassMethods
        def verified_stream_name_from_params
          StreamName.verified(params[:signed_stream_name])
        end
      end
    end
  end
end

# `Turbo::StreamsChannel` — turbo-rails' stock stream channel, the one a
# `<turbo-cable-stream-source>` names unless the page said otherwise.
#
# turbo-rails 2.0.16, whole class:
#
#   class Turbo::StreamsChannel < ActionCable::Channel::Base
#     include Turbo::Streams::StreamName
#     extend  Turbo::Streams::StreamName
#     def subscribed
#       if stream_name = verified_stream_name_from_params
#         stream_from stream_name
#       else
#         reject
#       end
#     end
#   end
#
# IT EXISTS FOR THE GUARD AS MUCH AS FOR THE SUBSCRIPTION. campfire
# prepends `RoomStreamsAreAuthorized` onto this class, and a prepend
# needs something to prepend ONTO: with no `Turbo::StreamsChannel` in
# the tree the mixin lowering drops the line and reports it, which is
# how an authorization module ends up defined, tested, and out of the
# lookup chain. `lower::module_mixins` credits this name for that
# reason, and `tests/initializer_module_mixins.rs` pins the two
# together.
#
# `super` from the prepended module lands HERE, which is why the body is
# the real thing rather than a marker class.
module Turbo
  class StreamsChannel < ActionCable::Channel::Base
    include Turbo::Streams::StreamName::ClassMethods

    def subscribed
      if stream_name = verified_stream_name_from_params
        stream_from stream_name
      else
        reject
      end
    end

    # ── the class-level broadcast API ────────────────────────────────
    #
    # THE SEAM A RAILS APP'S OWN TESTS MOCK. `broadcast_replace_to` is
    # where turbo-rails actually sends a stream, and an app asserting
    # "this action broadcast exactly once" stubs it — campfire's
    # `messages_controller_test` does it four times. Against an emitted
    # tree those four could not even reach their assertion while the
    # constant did not exist: the test died at `uninitialized constant
    # Turbo::StreamsChannel` before the request ran.
    #
    # ONE CONSTANT, BOTH HALVES, because that is what turbo-rails ships
    # and because the alternative does not load: these four lived in
    # `broadcasts.rb` under `module StreamsChannel` while the channel
    # above wanted `class StreamsChannel`, and Ruby answers that with
    # `StreamsChannel is not a class` at require time.
    #
    # ONLY THE RUBY FAMILY OWES THIS. The seam is reached from a TEST,
    # and the only tests that mock it are the app's own, which do not run
    # on a strict target — but the methods are plain delegations to
    # `Broadcasts.record`, so there is nothing here a strict target
    # cannot compile either.
    #
    # One hop, and `record` still owns the log: with no stub in place the
    # behaviour is byte-identical to calling `record` directly. With a
    # stub, nothing is logged — which is exactly what Rails does when the
    # channel is mocked out.
    def self.broadcast_append_to(stream, target:, html:, attributes: "")
      Broadcasts.record(action: :append, stream: stream, target: target, html: html, attributes: attributes)
    end

    def self.broadcast_prepend_to(stream, target:, html:, attributes: "")
      Broadcasts.record(action: :prepend, stream: stream, target: target, html: html, attributes: attributes)
    end

    def self.broadcast_replace_to(stream, target:, html:, attributes: "")
      Broadcasts.record(action: :replace, stream: stream, target: target, html: html, attributes: attributes)
    end

    def self.broadcast_remove_to(stream, target:, attributes: "")
      Broadcasts.record(action: :remove, stream: stream, target: target, html: "", attributes: attributes)
    end
  end
end
