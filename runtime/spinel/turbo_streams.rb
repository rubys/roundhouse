# `Turbo::Streams::StreamName` — the SIGNING half of Turbo's stream
# names, for a channel that guards its own stream.
#
# Turbo puts the stream name in the page, inside
# `<turbo-cable-stream-source signed-stream-name="…">`, and the client
# hands it straight back on subscribe. Rails HMAC-signs it on the way
# out and verifies on the way in, so a name the user edited is refused
# before it reaches a channel.
#
# THIS RUNTIME SIGNS, the way turbo-rails does. `Turbo.signed_stream
# _verifier` is
#
#   ActiveSupport::MessageVerifier.new(key, digest: "SHA256", serializer: JSON)
#   key = Rails.application.key_generator.generate_key("turbo/signed_stream_verifier_key")
#
# (turbo-rails 2.0.23, lib/turbo-rails.rb + lib/turbo/engine.rb), and a
# bare `MessageVerifier#generate` with no purpose and no expiry writes
# no `_rails` metadata envelope: the signed text is
#
#   strict_base64(JSON(name)) + "--" + hex(HMAC-SHA256(key, that base64))
#
# with the key PBKDF2-derived exactly as `ActionController::
# MessageVerifier.derive_key` derives every other Rails key in this
# runtime. Measured against campfire under Rails 8.2 rather than
# inferred — `test/turbo_streams_test.rb` pins the bytes Rails minted
# for a known secret — so a name this runtime writes into a page is one
# Rails would accept, and a name Rails signed verifies here.
#
# BOTH ENDS OF THE WIRE ARE IN THIS FILE: `StreamName.signed` is what
# `ActionView::ViewHelpers.turbo_stream_from` writes (reopened below,
# ruby family only — the shared helper in runtime/ruby writes the
# `--unsigned` placeholder for the targets with no verifier, see
# docs/pipeline/runtime.md), and `StreamName.verified` is what a
# subscribe reaches through the channel it named. A name the client
# edited fails the digest before any channel sees it.
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
      # turbo-rails' `signed_stream_verifier` is an ordinary
      # `MessageVerifier` keyed off this salt; the salt is the gem's, not
      # ours (lib/turbo/engine.rb).
      SALT = "turbo/signed_stream_verifier_key"

      # `Turbo.signed_stream_verifier.generate(name)`: the base64 of the
      # JSON-serialized name, signed. What `turbo_stream_from` writes
      # into the page and what `Turbo::StreamsChannel.signed_stream_name`
      # answers a test.
      def self.signed(name)
        payload = Base64.strict_encode64(JSON.generate(name))
        payload + "--" + ActionController::MessageVerifier.digest_for(
          Rails.application.secret_key_base, SALT, payload, false)
      end

      # `Turbo.signed_stream_verifier.verified(signed)`: the name back,
      # or nil for anything that does not verify — no `--`, a digest
      # that does not match, a payload that is not base64 or not a JSON
      # string. nil is what a caller checking `if stream_name =
      # verified_stream_name_from_params` expects; campfire's channel
      # rejects the subscription on it.
      #
      # A STRING OR NOTHING. Rails' verifier hands back whatever JSON
      # was signed; turbo only ever signs the `:`-joined name, so a
      # payload that decodes to anything else was not minted by
      # `signed` and reads as tampered.
      def self.verified(signed)
        return nil if signed.nil?
        text = signed.to_s
        sep = text.index("--")
        return nil if sep.nil? || sep == 0
        payload = text[0, sep]
        digest = text[sep + 2, text.length - sep - 2]
        expected = ActionController::MessageVerifier.digest_for(
          Rails.application.secret_key_base, SALT, payload, false)
        return nil if digest != expected
        begin
          value = JSON.parse(Base64.strict_decode64(payload))
        rescue ArgumentError, JSON::ParserError
          return nil
        end
        value.is_a?(String) ? value : nil
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

# The WRITER, reopened for the ruby family: `turbo_stream_from` in the
# shared `runtime/ruby/action_view/view_helpers.rb` spells the attribute
# through this one method, and the shared definition writes an
# `--unsigned` placeholder for the targets that have no verifier. Here
# the verifier exists, so the page carries the same bytes Rails would
# write and `StreamName.verified` above is the reader for them. Last
# definition wins on spinel as on CRuby; this file loads after
# action_view on both boot chains (boot.rb, test_helper.rb).
module ActionView
  module ViewHelpers
    def self.signed_stream_name(stream)
      Turbo::Streams::StreamName.signed(stream)
    end
  end
end

# `Turbo.signed_stream_verifier` — the object an app's own test asks
# to `verified` a name it minted (campfire's room_messages_channel_test
# hands the channel the VERIFIED, unsigned name to prove it is
# refused). turbo-rails memoizes a `MessageVerifier` here; this
# runtime's verifier is the pair of module functions above, so the
# object is a stateless facade over them.
module Turbo
  class SignedStreamVerifier
    def generate(name)
      Turbo::Streams::StreamName.signed(name)
    end

    def verified(signed)
      Turbo::Streams::StreamName.verified(signed)
    end
  end

  SIGNED_STREAM_VERIFIER = SignedStreamVerifier.new

  def self.signed_stream_verifier
    SIGNED_STREAM_VERIFIER
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

    # What `lower_channel_names` bakes into an app channel, spelled here
    # by hand because this class is the runtime's, not the app's:
    # `"Turbo::StreamsChannel".sub(/Channel$/, "").gsub("::", ":")
    # .underscore`.
    def channel_name
      "turbo:streams"
    end

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

    # The count slots `lower::mocha` writes for
    # `Turbo::StreamsChannel.expects(:broadcast_replace_to).once` (and
    # `_remove_to`). Single-element Arrays as settable holders, the
    # `Resolv` slot's idiom; -1 is "no count filed", since 0 is a real
    # expectation (`never`).
    #
    # With a count filed the call is COUNTED AND NOT RECORDED — mocha
    # replaces the method, so a mocked channel broadcasts nothing, and
    # a test that expects the broadcast does not also read the log. The
    # emitted helper clears in setup and verifies in teardown; an unmet
    # count raises there, charged to the test that filed it.
    REPLACE_CALLS = [ 0 ]
    REMOVE_CALLS = [ 0 ]
    REPLACE_EXPECTED = [ -1 ]
    REMOVE_EXPECTED = [ -1 ]

    def self.expect_broadcast_replace_to(count)
      REPLACE_EXPECTED[0] = count
      nil
    end

    def self.expect_broadcast_remove_to(count)
      REMOVE_EXPECTED[0] = count
      nil
    end

    def self.clear_broadcast_expectations
      REPLACE_CALLS[0] = 0
      REMOVE_CALLS[0] = 0
      REPLACE_EXPECTED[0] = -1
      REMOVE_EXPECTED[0] = -1
      nil
    end

    def self.verify_broadcast_expectations
      replace_expected = REPLACE_EXPECTED[0]
      remove_expected = REMOVE_EXPECTED[0]
      replace_got = REPLACE_CALLS[0]
      remove_got = REMOVE_CALLS[0]
      REPLACE_EXPECTED[0] = -1
      REMOVE_EXPECTED[0] = -1
      if replace_expected >= 0 && replace_got != replace_expected
        raise "Turbo::StreamsChannel.broadcast_replace_to was expected #{replace_expected} time(s), got #{replace_got}"
      end
      if remove_expected >= 0 && remove_got != remove_expected
        raise "Turbo::StreamsChannel.broadcast_remove_to was expected #{remove_expected} time(s), got #{remove_got}"
      end
      nil
    end

    def self.broadcast_replace_to(stream, target:, html:, attributes: "")
      if REPLACE_EXPECTED[0] >= 0
        REPLACE_CALLS[0] = REPLACE_CALLS[0] + 1
        return nil
      end
      Broadcasts.record(action: :replace, stream: stream, target: target, html: html, attributes: attributes)
    end

    # Turbo's `update` replaces a target's CONTENTS where `replace`
    # replaces the element itself. No call counter beside it: the
    # counters above exist for the replace/remove assertions the
    # broadcast tests make, and nothing asserts on update yet.
    def self.broadcast_update_to(stream, target:, html:, attributes: "")
      Broadcasts.record(action: :update, stream: stream, target: target, html: html, attributes: attributes)
    end

    def self.broadcast_remove_to(stream, target:, attributes: "")
      if REMOVE_EXPECTED[0] >= 0
        REMOVE_CALLS[0] = REMOVE_CALLS[0] + 1
        return nil
      end
      Broadcasts.record(action: :remove, stream: stream, target: target, html: "", attributes: attributes)
    end
  end
end
