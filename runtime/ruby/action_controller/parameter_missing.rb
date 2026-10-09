# ActionController::ParameterMissing — raised by `Params.require_key`
# when a required parameter is absent or blank, and RESCUED by app code
# as a normal path (a controller turns it into a 400 rather than letting
# a nil surface somewhere later). campfire's
# `unfurl_links_controller_test` asserts the raise by class, so the class
# has to be real rather than a message string.
#
# Ruby-family home (off the strict-target tables), same reasoning as
# `ActionView::MissingTemplate` beside it: exception classes as control
# flow ride the BeginRescue lowering, which the ruby-family targets and
# spinel AOT share.
module ActionController
  class ParameterMissing < StandardError
    def initialize(param)
      @param = param
      super("param is missing or the value is empty or invalid: #{param}")
    end

    def param
      @param
    end
  end

  # Named by `rescue_from` clauses (lobsters answers both with a 400 or a
  # 404). No runtime path raises either yet: strong-params filtering
  # does not reject unpermitted keys, and a format an action does not
  # answer is resolved by the lowering. They exist because a rescue
  # clause evaluates its class list whenever an exception passes
  # through it — an undefined name there turns every other error the
  # action raises into a NameError that hides the real one.
  class UnpermittedParameters < StandardError
  end

  class UnknownFormat < StandardError
  end

  # Raised by app code to answer 404 for a path that a route matches
  # but the app rejects (for example, a page number out of bounds).
  # Rails maps it to 404 in `ActionDispatch::ExceptionWrapper`, and the
  # ruby-family and spinel dispatchers rescue it beside
  # `ActiveRecord::RecordNotFound`. The constructor has the signature
  # of the Rails constructor: the message is required, and `failures`
  # is optional. Rails fills `failures` from its router. App code
  # passes an empty Array, so the RBS declares `Array[String]`.
  class RoutingError < StandardError
    def initialize(message, failures = [])
      @failures = failures
      super(message)
    end

    def failures
      @failures
    end
  end
end

# Raised by the emitted `process_action` when a route names an action
# the controller does not define (no method here or on an ancestor, and
# no template). Rails raises it from `AbstractController::Base#process`
# before the callback chain and outside `rescue_from`, and
# `ActionDispatch::ExceptionWrapper` maps it to 404; the ruby-family and
# spinel dispatchers rescue it beside `ActionController::RoutingError`.
module AbstractController
  class ActionNotFound < StandardError
  end
end
