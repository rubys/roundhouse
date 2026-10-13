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
