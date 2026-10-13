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

  def self.in_transaction
    Thread.current[:aj_in_transaction] == true
  end

  def self.enqueue_payload_after_commit(json)
    if in_transaction
      __held_until_commit << json
    else
      enqueue_payload(json)
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
    while held.length > 0
      enqueue_payload(held.shift)
    end
    nil
  end

  def self.transaction_rolled_back
    Thread.current[:aj_in_transaction] = false
    __held_until_commit.clear
    nil
  end

  # The drain's payload arm (`ActiveJob.drain` in the shared runtime and
  # its locked twin in thread_state.rb). REPLACES the shared default,
  # which knows no jobs: a later definition wins, on spinel as on CRuby.
  #
  # Answers whether a job ran. A record that is gone is
  # DeserializationError: dropped quietly when the job says
  # `discard_on ActiveJob::DeserializationError`, reported otherwise.
  # Any other error propagates to the drain, which reports it and moves
  # on, as it does for a Proc.
  def self.perform_payload(json)
    job = JSON.parse(json)
    name = job["job_class"].to_s
    begin
      ran = JobRegistry.perform(name, job["arguments"])
      warn "[job] no job class " + name + " for a queued payload" unless ran
      ran
    rescue ActiveJob::DeserializationError => e
      warn "[job] " + name + ": " + e.message unless JobRegistry.discards_deserialization_error(name)
      false
    end
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
