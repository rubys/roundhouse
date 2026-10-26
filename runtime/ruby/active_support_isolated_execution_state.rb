# frozen_string_literal: true
# `ActiveSupport::IsolatedExecutionState[key]` — a hash private to the
# current thread. Fiber-local, like `Thread.current` itself, and not
# inherited by a thread this one starts.
module ActiveSupport
  module IsolatedExecutionState
    def self.[](key)
      state[key]
    end

    def self.[]=(key, value)
      state[key] = value
    end

    def self.state
      current = Thread.current[:active_support_isolated_execution_state]
      return current unless current.nil?
      fresh = {}
      Thread.current[:active_support_isolated_execution_state] = fresh
      fresh
    end
  end
end
