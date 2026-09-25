# Exceptions Rails raises from the request layer, defined so the
# `rescue_from` clauses that name them resolve. Nothing in the runtime
# raises either: remote-IP spoof detection and Accept-header MIME
# validation are middleware work the emitted dispatcher does not do.
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
    module MimeNegotiation
      class InvalidType < StandardError
      end
    end
  end
end
