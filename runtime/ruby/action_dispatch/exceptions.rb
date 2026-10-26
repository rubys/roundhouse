# frozen_string_literal: true
# Exceptions Rails raises from the request layer, defined so the
# `rescue_from` clauses that name them resolve. Nothing in the runtime
# raises these: remote-IP spoof detection and Accept-header MIME
# validation are middleware work the emitted dispatcher does not do,
# and malformed JSON is rejected before controller dispatch.
# They exist because a rescue clause evaluates its class list whenever
# an exception passes through it, so an undefined name turns every
# other error the action raises into a NameError that hides the real
# one (lobsters' ApplicationController rescues both).
module ActionDispatch
  module RemoteIp
    class IpSpoofAttackError < StandardError
    end
  end

  module Http
    module Parameters
      class ParseError < StandardError
        # Omission wraps the active error; explicit nil keeps Exception's
        # class-name message. Rails also raises when no error is active.
        def initialize(message = $!.message)
          super(message)
        end
      end
    end

    module MimeNegotiation
      class InvalidType < StandardError
      end
    end
  end
end
