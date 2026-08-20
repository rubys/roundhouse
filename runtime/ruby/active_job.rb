# ActiveJob under the transpiled runtime: the adapter is `:inline` —
# there is no queue daemon in-process, so the class-side entries
# (`perform_later`, `set(...).perform_later`) are synthesized by
# `lower::job_class_side` to run `new.perform(...)` synchronously.
# This base class exists so `class ApplicationJob < ActiveJob::Base`
# resolves and the class-body DSL is inert.

module ActiveJob
  # Jobs that have RUN, by class name, in call order.
  #
  # The adapter is inline, so there is no queue to inspect — which is
  # also true of Rails' own `:inline` adapter, and why Rails' test
  # helpers require the `:test` one. This log is the equivalent seam:
  # `perform_later` appends here before dispatching, so
  # "jobs enqueued during this block" and "jobs run during this block"
  # are the same set. The divergence is a job enqueued and never run,
  # which inline semantics cannot produce.
  #
  # Same shape as `Broadcasts::LOG` and for the same reasons — a
  # module-level constant Array rather than a class ivar, because that
  # is the form every target carries. Entries are class NAMES, not
  # classes: a class is not a first-class value on the strict targets,
  # and the test call sites that name one are rewritten to the string
  # at compile time (`lower::job_test_only`).
  PERFORMED = []

  def self.record_performed(job_name)
    PERFORMED << job_name
    nil
  end

  # No reset: every helper reads a LENGTH DELTA across its block, the
  # same way `capture_turbo_stream_broadcasts` reads `Broadcasts::LOG`,
  # so the log growing across a file's tests is not a problem — and a
  # clear would hide what an earlier block in the same test did.
  def self.performed
    PERFORMED.dup
  end

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
