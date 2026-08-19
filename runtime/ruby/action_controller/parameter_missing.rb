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
end
