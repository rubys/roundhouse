# ActiveJob under the transpiled runtime: the adapter is `:inline` —
# there is no queue daemon in-process, so the class-side entries
# (`perform_later`, `set(...).perform_later`) are synthesized by
# `lower::job_class_side` to run `new.perform(...)` synchronously.
# This base class exists so `class ApplicationJob < ActiveJob::Base`
# resolves and the class-body DSL is inert.

module ActiveJob
  class Base
    # `queue_as :default` — queue routing has no meaning inline.
    def self.queue_as(name = nil)
      nil
    end

    # `self.enqueue_after_transaction_commit = true` — inline jobs run
    # at the call site; transaction-commit deferral is a queue concern.
    def self.enqueue_after_transaction_commit=(value)
      nil
    end

    # `retry_on` / `discard_on` — error-handling policy for queued
    # execution; inert inline (an inline job's exception propagates to
    # the caller, which is the honest development-mode behavior).
    def self.retry_on(error, opts = nil)
      nil
    end

    def self.discard_on(error)
      nil
    end
  end
end
