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
# `with_error` is the record Sidekiq 8.1 writes to the retry or dead
# set for a job that raised, rebuilt from the job's own text so every
# key it came with keeps its place and value:
#
#   - a first failure appends `error_message`, `error_class`,
#     `failed_at` and `retry_count: 0`;
#   - a job that already failed (it has `retry_count`, as one Ruby
#     Sidekiq retried does) keeps its `failed_at`, gets the new error,
#     `retried_at`, and `retry_count` one higher;
#   - `queue` becomes the queue it was popped from, or `retry_queue`
#     when the job names one.
#
# The message is cut to 10,000 characters, as Sidekiq cuts it. The same
# record goes to the retry set while the job has retries left
# (`exhausted?`), and to the dead set after.
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

  # Whether a failure goes through Sidekiq's retry rules: `retry: false`
  # (and no `retry` at all) drops the job instead, writing nothing.
  # `retry: 0` is truthy in Ruby, so it takes the rules and is exhausted
  # at once.
  def self.retries?(env)
    r = env["retry"]
    !(r.nil? || r == false)
  end

  # The retry_count the failure writes: 0 for a first failure, else one
  # more than the job carries.
  def self.next_retry_count(env)
    count = env["retry_count"]
    count.nil? ? 0 : count.to_i + 1
  end

  # Whether this failure, the `count`th retry, is the last: past
  # `retry_for` seconds since the first failure when the job has
  # `retry_for` (count then plays no part), else at the limit, which is an
  # Integer `retry` or `max_retries`. Sidekiq's `process_retry`.
  def self.exhausted?(env, count, max_retries, now_ms)
    rf = env["retry_for"]
    unless rf.nil?
      return first_failed_seconds(env, now_ms) + rf.to_f < now_ms / 1000.0
    end
    r = env["retry"]
    limit = r.is_a?(Integer) ? r : max_retries
    count >= limit
  end

  # When the job first failed, in seconds: `failed_at` as Integer
  # milliseconds (or Float seconds, as Sidekiq 6 wrote it), or now for a
  # first failure.
  def self.first_failed_seconds(env, now_ms)
    at = env["failed_at"]
    return now_ms / 1000.0 if at.nil?
    return at.to_f if at.is_a?(Float)
    at.to_i / 1000.0
  end

  # Whether an exhausted job is dropped rather than written to the dead
  # set: it says `dead: false`.
  def self.discard_when_exhausted?(env)
    env["dead"] == false
  end

  # Seconds until the `count`th retry: Sidekiq's default backoff,
  # `count**4 + 15` plus up to `10 * (count + 1)` of jitter.
  def self.retry_delay(count)
    count**4 + 15 + rand(10 * (count + 1))
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

  # The job's text with `enqueued_at` set to `now_ms`, in place when it
  # has one (a retry) and appended when it doesn't (a job from the
  # schedule set, which Sidekiq's push wrote without one), as
  # Sidekiq's client does when the scheduler pushes the job again.
  def self.with_enqueued_at(raw, now_ms)
    env = JSON.parse(raw)
    env["enqueued_at"] = now_ms
    JSON.generate(env)
  end

  def self.truncate(message)
    return message if message.length <= ERROR_MESSAGE_LIMIT
    message[0, ERROR_MESSAGE_LIMIT]
  end
end
