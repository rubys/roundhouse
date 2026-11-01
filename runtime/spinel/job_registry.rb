# frozen_string_literal: true
# The app's ActiveJob payload jobs, by name: how each one's arguments
# are written at `perform_later` and read back when the drain runs it.
#
# GENERATED between the markers — `project::apply_job_registry` writes
# it from `App::job_plans` (see `lower::job_payload`), the same
# marker-span shape `global_id_locator.rb` uses. Per payload job:
#
#   def self.payload_room__push_message_job(room, message)
#     ActiveJob::Payload.build("Room::PushMessageJob", "default",
#       ActiveJob::Arguments.list([ActiveJob::Arguments.record(room.to_gid_uri), …]))
#   end
#
# which the job's `perform_later` hands to `ActiveJob.enqueue_payload`;
# a `when "Room::PushMessageJob"` arm in `perform` that reads each
# argument back with its typed reader and calls `perform`; and, per
# record model a job reads, `locate_<model>_at`, which accepts the model
# or its STI subclasses BY LITERAL NAME (a name from the wire is never
# turned into a class) and raises DeserializationError for a row that is
# gone, as Rails does.
#
# Under `queue_adapter :sidekiq`, `handler_for` and `retry_rule` are the
# jobs' `retry_on` and `discard_on` handlers (`ActiveJob.perform_parsed_from`
# reads them), and `sidekiq_retry` and `sidekiq_extra` their
# `sidekiq_options` (`SidekiqPush.enqueue` writes them).
#
# `sidekiq?` says whether the app is on `queue_adapter :sidekiq` with a
# payload job, so the tree pushes jobs to Redis (`runtime/sidekiq_push.rb`)
# and carries a real Redis client. `sidekiq_redis_url` and
# `sidekiq_redis_password` are the app's `Sidekiq.configure_client`
# settings, `""` for one it doesn't give.
#
# Dispatch is a `case` over the class name, never `constantize`, for
# the reason the locator gives: a class computed from a wire string is
# a shape this pipeline will not emit.
module JobRegistry
  # >>> generated: job-registry
  def self.perform(job_class, args)
    false
  end

  def self.discards_deserialization_error(job_class)
    false
  end

  def self.handler_for(job_class, e)
    0
  end

  def self.retry_rule(id)
    ActiveJob::RetryRule.new("", 0.0, false, 0, "", false, 0, 0.0)
  end

  def self.sidekiq_retry(job_class)
    "true"
  end

  def self.sidekiq_extra(job_class)
    ""
  end

  def self.sidekiq?
    false
  end

  def self.sidekiq_redis_url
    ""
  end

  def self.sidekiq_redis_password
    ""
  end

  def self.sidekiq_worker_concurrency
    0
  end

  def self.sidekiq_worker_timeout
    -1
  end

  def self.sidekiq_worker_queues
    ""
  end

  def self.sidekiq_worker_max_retries
    -1
  end

  def self.sidekiq_worker_dead_max_jobs
    -1
  end

  def self.sidekiq_worker_dead_timeout
    -1
  end

  def self.sidekiq_worker_poll_interval_average
    -1.0
  end

  def self.sidekiq_worker_scheduled_poll_interval
    -1.0
  end

  def self.cable_redis?
    false
  end

  def self.cable_redis_url
    ""
  end

  def self.cable_channel_prefix
    ""
  end
  # <<< generated: job-registry
end

module ActiveJob
  # ---- Enqueue after commit ------------------------------------------
  #
  # A job that sets `self.enqueue_after_transaction_commit = true` (on
  # itself or `ApplicationJob`, as Lobsters does) is not queued while a
  # transaction is open: the drain runs on its own connection and would
  # look the record up before the COMMIT made it visible, then raise
  # DeserializationError for a row that is about to exist. Rails holds
  # it the same way. `Model.transaction` reports its edges here, through
  # `ActiveRecord::TransactionHooks` (below); held payloads are queued at COMMIT and
  # dropped at ROLLBACK. Rails 8.1's default for the setting is false,
  # and a job that keeps it so is queued at once, as before.
  #
  # Under `queue_adapter :sidekiq` the held jobs are Sidekiq pushes,
  # kept in `SidekiqPush.held` and pushed or dropped at the same edges.
  #
  # Per thread, because the transaction is: each request thread holds
  # its own connection lease. Only the outermost transaction counts: a
  # job enqueued inside a nested `requires_new:` block that rolls back
  # to its SAVEPOINT is still queued at the outer COMMIT.
  def self.__held_until_commit
    held = Thread.current[:aj_held_until_commit]
    if held.nil?
      held = [""]
      held.clear
      Thread.current[:aj_held_until_commit] = held
    end
    held
  end

  def self.__held_queues_until_commit
    held = Thread.current[:aj_held_queues_until_commit]
    if held.nil?
      held = [""]
      held.clear
      Thread.current[:aj_held_queues_until_commit] = held
    end
    held
  end

  def self.in_transaction
    Thread.current[:aj_in_transaction] == true
  end

  def self.enqueue_payload_after_commit_on(queue, json)
    if in_transaction
      __held_queues_until_commit << queue
      __held_until_commit << json
    else
      enqueue_payload_on(queue, json)
    end
    nil
  end

  def self.transaction_began
    Thread.current[:aj_in_transaction] = true
    nil
  end

  def self.transaction_committed
    Thread.current[:aj_in_transaction] = false
    held = __held_until_commit
    queues = __held_queues_until_commit
    while held.length > 0
      enqueue_payload_on(queues.shift.to_s, held.shift.to_s)
    end
    SidekiqPush.flush_held
  end

  def self.transaction_rolled_back
    Thread.current[:aj_in_transaction] = false
    __held_until_commit.clear
    __held_queues_until_commit.clear
    SidekiqPush.drop_held
  end

  # What `perform_parsed` answers. An Integer rather than a raise, so
  # the worker's loop needs no rescue for an app's exceptions: the job's
  # error is caught here, and its class and message are kept for the
  # thread that ran it (`failure_class`, `failure_message`).
  PERFORM_RAN = 0
  PERFORM_DISCARDED = 1
  PERFORM_UNKNOWN_CLASS = 2
  PERFORM_FAILED = 3
  # A `retry_on` caught the error and pushed the job again.
  PERFORM_RETRIED = 4

  # One `retry_on`: the key its tries are counted under in
  # `exception_executions` (the exception list's `to_s`, "[Sticky]"),
  # its wait in seconds or `polynomially_longer`, its attempts (0 for
  # `:unlimited`), the queue and priority it pushes to ("" and no
  # priority to keep the job's), and its jitter.
  class RetryRule
    attr_reader :key, :wait, :polynomially_longer, :attempts, :queue, :has_priority, :priority, :jitter

    def initialize(key, wait, polynomially_longer, attempts, queue, has_priority, priority, jitter)
      @key = key
      @wait = wait
      @polynomially_longer = polynomially_longer
      @attempts = attempts
      @queue = queue
      @has_priority = has_priority
      @priority = priority
      @jitter = jitter
    end
  end

  def self.perform_parsed(job)
    perform_parsed_from(job, "")
  end

  # Run one parsed ActiveJob payload, popped from Sidekiq as `jid` ("" in
  # process). An error goes to the job's handlers first, as ActiveJob's
  # `rescue_from` does: a `discard_on` is DISCARDED, and a `retry_on`
  # with tries left pushes the job again and is RETRIED. In process a
  # `retry_on` does nothing, and a record that is gone is DISCARDED when
  # the job says `discard_on ActiveJob::DeserializationError`. Otherwise
  # FAILED, with the error's class (ActiveJob::DeserializationError for
  # a record that is gone).
  def self.perform_parsed_from(job, jid)
    name = job["job_class"].to_s
    begin
      if JobRegistry.perform(name, job["arguments"])
        PERFORM_RAN
      else
        PERFORM_UNKNOWN_CLASS
      end
    rescue Exception => e
      # ActiveJob's perform_now rescues Exception for `rescue_from`, and
      # re-raises what no handler takes; the worker sees only a
      # StandardError as a job failure.
      gone = e.is_a?(ActiveJob::DeserializationError)
      handler = JobRegistry.handler_for(name, e)
      if handler < 0 || (handler == 0 && gone && JobRegistry.discards_deserialization_error(name))
        PERFORM_DISCARDED
      elsif handler > 0 && jid != "" && retry_job(job, jid, JobRegistry.retry_rule(handler))
        PERFORM_RETRIED
      elsif !e.is_a?(StandardError)
        raise e
      elsif gone
        note_failure("ActiveJob::DeserializationError", e.message)
      else
        note_failure(e.class.name.to_s, e.message)
      end
    end
  end

  # ActiveJob's `retry_on` for a job that raised: count the try under
  # the rule's key and, while tries are left, push the job again after
  # the rule's wait, as `retry_job` does. False when the tries are used
  # up, so the error goes on to Sidekiq's own retry rules.
  def self.retry_job(job, jid, rule)
    # ActiveJob's `executions_for`: a job enqueued before per-handler
    # counters existed counts its executions instead.
    counts = job["exception_executions"]
    tries = (counts.is_a?(Hash) ? counts[rule.key].to_i : job["executions"].to_i) + 1
    return false if rule.attempts > 0 && tries >= rule.attempts
    counts[rule.key] = tries if counts.is_a?(Hash)
    job["executions"] = job["executions"].to_i + 1
    job["provider_job_id"] = jid
    job["queue_name"] = rule.queue unless rule.queue == ""
    job["priority"] = rule.priority if rule.has_priority
    now = Time.now.to_f
    at = now + retry_delay(rule, tries)
    job["enqueued_at"] = Time.at(now).utc.iso8601(9)
    job["scheduled_at"] = Time.at(at).utc.iso8601(9)
    name = job["job_class"].to_s
    SidekiqPush.enqueue(name, job["queue_name"].to_s, at, JSON.generate(job))
    true
  end

  # ActiveJob's `determine_delay`: a fixed wait, or `polynomially_longer`'s
  # `tries**4 + 2`, plus up to `jitter` of the wait at random.
  def self.retry_delay(rule, tries)
    if rule.polynomially_longer
      delay = (tries**4).to_f
      delay + retry_jitter(delay, rule.jitter) + 2.0
    else
      rule.wait + retry_jitter(rule.wait, rule.jitter)
    end
  end

  def self.retry_jitter(delay, jitter)
    return 0.0 if jitter == 0.0
    retry_rand * delay * jitter
  end

  # `Kernel.rand`, the seam a test pins.
  def self.retry_rand
    rand
  end

  def self.note_failure(error_class, message)
    Thread.current[:aj_failure_class] = error_class
    Thread.current[:aj_failure_message] = message
    PERFORM_FAILED
  end

  def self.failure_class
    Thread.current[:aj_failure_class].to_s
  end

  def self.failure_message
    Thread.current[:aj_failure_message].to_s
  end

  # The drain's payload arm (`ActiveJob.drain` in the shared runtime and
  # its locked twin in thread_state.rb). REPLACES the shared default,
  # which knows no jobs: a later definition wins, on spinel as on CRuby.
  #
  # Answers whether a job ran. A record that is gone is reported unless
  # the job discards it. Any other error is raised again for the drain,
  # which reports its message and moves on, as it does for a Proc.
  def self.perform_payload(json)
    job = JSON.parse(json)
    name = job["job_class"].to_s
    outcome = perform_parsed(job)
    if outcome == PERFORM_UNKNOWN_CLASS
      warn "[job] no job class " + name + " for a queued payload"
    elsif outcome == PERFORM_FAILED
      raise failure_message unless failure_class == "ActiveJob::DeserializationError"
      warn "[job] " + name + ": " + failure_message
    end
    outcome == PERFORM_RAN
  end
end

# `Model.transaction`'s edges (no-ops in active_record/connection.rb),
# routed to the enqueue-after-commit hold above.
module ActiveRecord
  module TransactionHooks
    def self.began
      ActiveJob.transaction_began
    end

    def self.committed
      ActiveJob.transaction_committed
    end

    def self.rolled_back
      ActiveJob.transaction_rolled_back
    end
  end
end
