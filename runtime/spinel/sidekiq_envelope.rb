# frozen_string_literal: true
# Reading and rewriting the Sidekiq job envelopes a worker pops from
# Redis: the ones `runtime/sidekiq_push.rb` writes, and the ones a Rails
# app's Sidekiq adapter writes.
#
#   {"retry":true,"queue":"default","wrapped":"InspectCarJob",
#    "args":[<ActiveJob payload>],"class":"Sidekiq::ActiveJob::Wrapper",
#    "jid":"<24 hex>","created_at":<ms>,"enqueued_at":<ms>}
#
# The readers take the envelope already parsed (`JSON.parse` output, so
# `untyped`, the same contract as `ActiveJob::Arguments`' readers): the
# worker parses each job once. `aj_payload` is what
# `ActiveJob.perform_parsed` runs.
#
# `with_error` is the record Sidekiq 8.1 writes to the dead set (and,
# later, the retry set) for a job that raised, rebuilt from the job's
# own text so every key it came with keeps its place and value:
#
#   - a first failure appends `error_message`, `error_class`,
#     `failed_at` and `retry_count: 0`;
#   - a job that already failed (it has `retry_count`, as one Ruby
#     Sidekiq retried does) keeps its `failed_at`, gets the new error,
#     `retried_at`, and `retry_count` one higher;
#   - `queue` becomes the queue it was popped from, or `retry_queue`
#     when the job names one.
#
# The message is cut to 10,000 characters, as Sidekiq cuts it.
# `tests/sidekiq_worker.rs` holds the output to the records real
# Sidekiq 8.1.7 wrote (`tests/sidekiq_worker/sidekiq.json`).
module SidekiqEnvelope
  # Sidekiq 8's name for the ActiveJob wrapper, and the older alias that
  # jobs pushed by Sidekiq 7 still carry.
  WRAPPER = "Sidekiq::ActiveJob::Wrapper"
  LEGACY_WRAPPER = "ActiveJob::QueueAdapters::SidekiqAdapter::JobWrapper"
  ERROR_MESSAGE_LIMIT = 10_000

  def self.klass(env)
    env["class"].to_s
  end

  def self.jid(env)
    env["jid"].to_s
  end

  def self.queue(env)
    env["queue"].to_s
  end

  # Whether the job is an ActiveJob job, which this binary can run.
  def self.active_job?(env)
    k = klass(env)
    k == WRAPPER || k == LEGACY_WRAPPER
  end

  # The ActiveJob payload: `job_class`, `arguments` and the rest.
  def self.aj_payload(env)
    env["args"][0]
  end

  # Whether Sidekiq would keep this job in the dead set once it has
  # failed for good: `retry: false` jobs are dropped, and so are jobs
  # that say `dead: false`.
  def self.dead?(env)
    r = env["retry"]
    return false if r.nil? || r == false
    env["dead"] != false
  end

  # The text the job is written back as, after a shutdown cut it short:
  # the bytes it was popped as, unchanged, as Sidekiq's requeue does.
  def self.requeue_text(raw)
    raw
  end

  def self.with_error(raw, queue, error_class, message, now_ms)
    env = JSON.parse(raw)
    retry_queue = env["retry_queue"]
    env["queue"] = retry_queue.nil? ? queue : retry_queue.to_s
    env["error_message"] = truncate(message)
    env["error_class"] = error_class
    if !env["retry_count"].nil?
      env["retried_at"] = now_ms
      env["retry_count"] = env["retry_count"].to_i + 1
    else
      env["failed_at"] = now_ms
      env["retry_count"] = 0
    end
    JSON.generate(env)
  end

  def self.truncate(message)
    return message if message.length <= ERROR_MESSAGE_LIMIT
    message[0, ERROR_MESSAGE_LIMIT]
  end
end
