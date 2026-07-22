# Per-request context reachable from module-function helpers. Rails
# helpers run in the view context, which delegates `request` to the
# controller; the emitted helpers are module functions with no such
# context, so the dispatcher parks the request/controller here (the
# ActiveSupport::CurrentAttributes pattern) and the Ruby emit path
# rewrites bare `request` reads in helper/view module bodies to
# `ActionController::Current.request`. Single-threaded dispatch —
# plain module state, reset by assignment each request.
#
# Module-ivar statics rather than `class << self` (the overlay's
# shape): explicit `def self.` accessors are the statically-resolvable
# form every ingest/AOT path handles. The controller is parked (not
# its session) because `reset_session` swaps the controller's @session
# mid-action — a parked session reference would go stale.
module ActionController
  class Base
    attr_accessor :request
  end

  module Current
    def self.request
      @request
    end

    def self.request=(value)
      @request = value
    end

    def self.controller
      @controller
    end

    def self.controller=(value)
      @controller = value
    end

    # The current request's session, or nil outside a dispatch (unit
    # tests construct view helpers without a controller; they get the
    # shared runtime's empty-token behavior).
    def self.session
      c = @controller
      return nil if c.nil?
      c.session
    end
  end
end
