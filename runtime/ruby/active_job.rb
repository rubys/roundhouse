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

  # ---- A real queue -------------------------------------------------
  #
  # `perform_later` PUTS THE WORK DOWN AND RETURNS, which is what Rails
  # means by it. Everything above this describes the two postures that
  # came before: run at the call site, or skip. Neither is what Rails
  # does, and the difference showed up the moment a real app was driven
  # rather than tested — campfire's `Message` has `after_create_commit
  # -> { room.receive(self) }` whose tail is a `perform_later`, so
  # posting a message ran web-push delivery INSIDE the POST. On the
  # spinel binary, where an unhandled error ends the process rather
  # than the request, that is a 500 at best.
  #
  # A ZERO-ARGUMENT PROC IS THE ENTRY, and that is what makes this
  # expressible on a strict target at all. A queue of jobs is
  # heterogeneous by nature — `Room::PushMessageJob` takes (room,
  # message), another takes an Integer — and there is nowhere to put
  # that on a target with one element type per container. A Proc
  # closing over its own arguments erases the difference: every entry
  # is `() -> nil`, whatever it closed over. Probed on spinel before
  # this was written, with two job classes of different arities and
  # argument types, and the output is byte-identical to CRuby's.
  #
  # ONE QUEUE, NOT ONE PER QUEUE NAME. `queue_as` is inert here (see
  # `Base` below) because a single in-process drain has no scheduling
  # decision to make: there is no worker pool to allocate and no
  # priority to honour, so recording the name would suggest an ordering
  # guarantee nothing implements.
  PENDING = []

  # Whether a drain exists to pick the work up. Empty means no — and
  # then `perform_later` runs inline, exactly as before, because a job
  # put on a queue nobody drains is a job silently dropped, which is
  # the failure mode this whole file is trying not to have.
  #
  # `main.rb` opts in by spawning the drain; the emitted test harness
  # does not, so the suite keeps the inline semantics its assertions
  # were written against.
  DRAINED = []

  def self.drain_registered
    DRAINED.length > 0
  end

  def self.register_drain
    DRAINED << "1"
    nil
  end

  # THE PROC ARRIVES AS AN ARGUMENT, NOT A BLOCK, and the reason is the
  # runtime's own invariant rather than taste: RBS has no way to name a
  # block parameter, so `&work` cannot be given a type and
  # `tests/runtime_src_integration.rs` fails the file for an untyped
  # sub-expression.
  #
  # It is declared `untyped` rather than `^() -> nil` only because the
  # RBS reader does not support ProcType yet ("unsupported RBS type
  # node: ProcType"). The precise spelling is the one this wants, and a
  # Proc is opaque to the analyzer either way -- `work.call` answers
  # gradually under both.
  def self.enqueue(work)
    PENDING << work
    nil
  end

  # ---- Serialized payloads --------------------------------------------
  #
  # An ActiveJob-format JSON payload (the `job.serialize` hash Rails
  # hands to Sidekiq), built by `ActiveJob::Payload.build` on the
  # Ruby-family lanes. A job whose arguments all serialize goes this
  # way, and the drain looks its records up again when it runs, as
  # Rails does. Every other job keeps a Proc over its live arguments.
  #
  # THE PAYLOAD RIDES THE SAME QUEUE, wrapped in a Proc. A strict target
  # has one element type per Array, so `PENDING` cannot hold a String
  # beside its Procs; a Proc that closes over the JSON and runs it
  # through `perform_payload` is the same `() -> nil` as every other
  # entry, so the one FIFO and its locking stay as they are. What the
  # Proc closes over is the serialized text, not the job's objects.
  def self.enqueue_payload(json)
    enqueue(-> { perform_payload(json)
    nil })
  end

  def self.pending_count
    PENDING.length
  end

  # Run one payload and answer whether a job class took it. The default
  # knows no job classes: only the Ruby-family lanes build payloads, and
  # their job registry redefines this (a later definition wins, as in
  # `thread_state`). A payload reaching this one is reported, not run.
  def self.perform_payload(json)
    warn "[job] no job registry for a queued payload"
    false
  end

  # Raised when a payload names a record that is gone, as Rails' is.
  # `discard_on ActiveJob::DeserializationError` drops the job quietly.
  class DeserializationError < StandardError
  end

  # Run every job queued so far, FIFO, and answer how many ran.
  #
  # A RAISE IS ONE LOST JOB, NEVER A LOST DRAIN, and on the spinel lane
  # never a lost process: there is no per-request rescue under tep, so
  # an unhandled error in a job would take every other connection with
  # it. The job's own failure is reported where an operator can see it
  # — Rails logs the same event — and the queue moves on.
  #
  # A job that enqueues another is handled by the `while`: the new
  # entry is behind the current one and this pass reaches it. That is
  # Rails' behaviour for an inline drain too.
  def self.drain
    ran = 0
    while PENDING.length > 0
      work = PENDING.shift
      begin
        work.call
        ran = ran + 1
      rescue StandardError => e
        warn "[job] a queued job raised: " + e.message
      end
    end
    ran
  end

  # ---- Held by the `:test` adapter ------------------------------------
  #
  # Rails' `:test` adapter keeps what was enqueued, and a test can run it
  # afterwards with a blockless `perform_enqueued_jobs`: campfire's
  # "the unread fanout skips a member who has opened the room since"
  # (basecamp/once-campfire#296) posts, changes the member's state, and
  # only then performs the job. So `perform_later` under `enqueue_only`
  # holds its work, the same zero-argument Proc a drain would take, with
  # the job's name beside it for `only:`.
  HELD_NAMES = []
  HELD = []

  def self.hold(job_name, work)
    HELD_NAMES << job_name
    HELD << work
    nil
  end

  # Run the held jobs `only` names (all of them when it is empty), in
  # the order they were enqueued, and answer how many ran. A job one of
  # them enqueues is held behind it and runs in the same pass. Jobs
  # `only` leaves out stay held.
  def self.perform_held(only)
    ran = 0
    i = 0
    while i < HELD.length
      if only.empty? || only.include?(HELD_NAMES[i])
        work = HELD[i]
        HELD.delete_at(i)
        HELD_NAMES.delete_at(i)
        work.call
        ran = ran + 1
      else
        i = i + 1
      end
    end
    ran
  end

  # Rails' test adapter starts every test with nothing enqueued.
  def self.clear_held
    HELD.clear
    HELD_NAMES.clear
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
