# ActiveJob under the transpiled runtime: the adapter is `:inline` —
# there is no queue daemon in-process, so the class-side entries
# (`perform_later`, `set(...).perform_later`) are synthesized by
# `lower::job_class_side` to run `new.perform(...)` synchronously.
# Under test it is `:test` instead, which enqueues without running;
# see `ENQUEUE_ONLY` below for why that difference is load-bearing.
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

  # ---- The `:test` adapter -------------------------------------------
  #
  # Rails' test environment does NOT use the inline adapter: it uses
  # `:test`, which ENQUEUES and does not run. That distinction is
  # load-bearing, not cosmetic. campfire's `Message` has
  # `after_create_commit -> { room.receive(self) }`, whose tail is
  # `Room::PushMessageJob.perform_later` — under inline dispatch EVERY
  # message a fixture loads would run `Room::MessagePusher`, and the
  # whole suite dies in its unresolvable nested join. Rails' own suite
  # never reaches that code because its adapter enqueues.
  #
  # A SUSPENSION STACK, not a boolean: `perform_enqueued_jobs { … }`
  # nests, and a depth counter is what makes an inner block restore the
  # outer state rather than clobber it. Spelled as a module-level
  # constant Array read through `length` — the same form `PERFORMED`
  # and `Broadcasts::LOG` take, and for the same reason: an Array
  # constant is the only mutable module state every target carries (a
  # `[0] =` index assignment is a shape several emitters do not).
  #
  # Empty is the default, so nothing changes for an app: `main.rb`
  # never touches this and its jobs still run at the call site. The
  # emitted test harness pushes once at boot.
  ENQUEUE_ONLY = []

  # Named without a `?`: predicate mangling for a MODULE function is
  # not uniform across targets (rust2 makes `<name>_pred`, the TS
  # Module mode emits a bare `?` and fails to parse). The same reason
  # `Params.provided` carries the name it does.
  def self.enqueue_only
    ENQUEUE_ONLY.length > 0
  end

  # Switch to the `:test` adapter — `perform_later` records and returns
  # without dispatching.
  def self.enqueue_without_running
    ENQUEUE_ONLY << "1"
    nil
  end

  # Back to inline dispatch. `perform_enqueued_jobs { … }` brackets its
  # block with this and `enqueue_without_running`, which is the honest
  # reading of Rails' "drain the queue now": we hold no arguments to
  # replay, so the jobs the block enqueues run as it enqueues them.
  def self.run_enqueued
    ENQUEUE_ONLY.pop
    nil
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
