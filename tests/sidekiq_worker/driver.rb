# The worker's envelope handling, written by the runtime a Sidekiq
# worker runs: runtime/spinel/sidekiq_envelope.rb over
# `ActiveJob.perform_parsed` in runtime/spinel/job_registry.rb.
#
# tests/sidekiq_worker.rs copies this file beside the runtime sources it
# requires, writes popped.txt (the jobs real Sidekiq popped in the
# oracle's strict scenario, one per line, per-run values pinned), then
# runs it under CRuby and, compiled by spinel, as a binary. It prints:
#
#   dead <json>   for each popped job that fails, as the worker would
#                 write it to the dead set at NOW_MS
#   then one `ok`/`FAIL` line per check
#
# JobRegistry.perform is replaced below with the oracle's jobs, so no
# app is involved.
require "json"
require "securerandom"
require "time"
require_relative "base64"

module Rails
  class App
    def global_id_app
      "depot"
    end

    def secret_key_base
      ""
    end
  end

  def self.application
    App.new
  end
end

module ActionController
  class MessageVerifier
    def self.verified_data_json(secret, salt, data, purpose, rotate)
      ""
    end

    def self.json_value(json)
      ""
    end
  end
end

require_relative "active_job"
require_relative "global_id_locator"
require_relative "active_job_serialization"
require_relative "sidekiq_redis"
require_relative "sidekiq_push"
require_relative "job_registry"
require_relative "sidekiq_envelope"

# The oracle's jobs (tests/sidekiq_worker/oracle_jobs.rb), plus the
# Phase 2 push jobs and two whose record is gone.
module JobRegistry
  RAN = [""]

  def self.perform(job_class, args)
    if job_class == "BoomJob"
      raise ArgumentError, "boom \"" + ActiveJob::Arguments.str_at(args, 0) + "\"\nsecond line"
    elsif job_class == "GoneJob" || job_class == "GoneQuietJob"
      raise ActiveJob::DeserializationError, "Couldn't find Car with 'id'=9"
    elsif job_class == "RecordJob" || job_class == "InspectCarJob" || job_class == "WashCarJob"
      RAN.push(job_class + " " + ActiveJob::Arguments.arg_count(args).to_s)
      true
    else
      false
    end
  end

  def self.discards_deserialization_error(job_class)
    job_class == "GoneQuietJob"
  end
end

NOW_MS = 1_792_632_731_000

def check(name, ok, detail)
  puts (ok ? "ok   " : "FAIL ") + name + (ok ? "" : " (" + detail + ")")
end

# What the worker does with one popped ActiveJob job: run it, and write
# the dead record when it fails.
def run_popped(queue, raw)
  env = JSON.parse(raw)
  return "not active job" unless SidekiqEnvelope.active_job?(env)
  outcome = ActiveJob.perform_parsed(SidekiqEnvelope.aj_payload(env))
  if outcome == ActiveJob::PERFORM_FAILED
    puts "dead " + SidekiqEnvelope.with_error(raw, queue,
      ActiveJob.failure_class, ActiveJob.failure_message, NOW_MS)
  end
  outcome.to_s
end

File.read("popped.txt").split("\n").each do |raw|
  next unless raw.start_with?("{\"retry\"")
  run_popped("high", raw)
end
check("RecordJob ran from the popped text", JobRegistry::RAN.include?("RecordJob 1"), JobRegistry::RAN.join(","))

# Every Phase 2 push envelope, written by SidekiqPush and read back.
SidekiqPush.start_recording
SidekiqPush.enqueue("InspectCarJob", "default", 0.0,
  ActiveJob::Payload.build_at("InspectCarJob", "default", 0.0,
    ActiveJob::Arguments.list([ActiveJob::Arguments.record("gid://depot/Car/1")])))
SidekiqPush.enqueue("WashCarJob", "low", Time.now.to_f + 300.0,
  ActiveJob::Payload.build_at("WashCarJob", "low", Time.now.to_f + 300.0,
    ActiveJob::Arguments.list([ActiveJob::Arguments.record("gid://depot/Car/2"), ActiveJob::Arguments.int(3)])))
envelopes = []
SidekiqPush.recorded.each do |line|
  parts = line.split(" ", 3)
  if parts[0] == "lpush"
    envelopes.push(parts[2])
  elsif parts[0] == "zadd"
    envelopes.push(parts[2][parts[2].index(" ") + 1, parts[2].length])
  end
end
SidekiqPush.stop_recording
check("two envelopes pushed", envelopes.length == 2, envelopes.length.to_s)
first = JSON.parse(envelopes[0])
check("klass", SidekiqEnvelope.klass(first) == "Sidekiq::ActiveJob::Wrapper", SidekiqEnvelope.klass(first))
check("queue", SidekiqEnvelope.queue(first) == "default", SidekiqEnvelope.queue(first))
check("jid", SidekiqEnvelope.jid(first).length == 24, SidekiqEnvelope.jid(first))
check("active job", SidekiqEnvelope.active_job?(first), "")
check("retried on failure", SidekiqEnvelope.retries?(first), "")
JobRegistry::RAN.clear
check("pushed job runs", run_popped("default", envelopes[0]) == "0", "")
check("scheduled job runs", run_popped("low", envelopes[1]) == "0", "")
check("with its arguments", JobRegistry::RAN.join(",") == "InspectCarJob 1,WashCarJob 2", JobRegistry::RAN.join(","))
check("requeue text is the popped text", SidekiqEnvelope.requeue_text(envelopes[0]) == envelopes[0], "")

# Outcomes.
def payload(job_class)
  JSON.parse(ActiveJob::Payload.build(job_class, "default",
    ActiveJob::Arguments.list([ActiveJob::Arguments.str("x")])))
end
check("unknown class", ActiveJob.perform_parsed(payload("MailerJob")) == ActiveJob::PERFORM_UNKNOWN_CLASS, "")
check("discarded", ActiveJob.perform_parsed(payload("GoneQuietJob")) == ActiveJob::PERFORM_DISCARDED, "")
check("gone record fails", ActiveJob.perform_parsed(payload("GoneJob")) == ActiveJob::PERFORM_FAILED, "")
check("as DeserializationError", ActiveJob.failure_class == "ActiveJob::DeserializationError", ActiveJob.failure_class)
check("raise fails", ActiveJob.perform_parsed(payload("BoomJob")) == ActiveJob::PERFORM_FAILED, "")
check("with its class", ActiveJob.failure_class == "ArgumentError", ActiveJob.failure_class)
check("and message", ActiveJob.failure_message == "boom \"x\"\nsecond line", ActiveJob.failure_message)

# Sidekiq's retry rules, read from the envelope.
def env(json)
  JSON.parse(json)
end
check("retry: false is dropped", !SidekiqEnvelope.retries?(env("{\"retry\":false}")), "")
check("no retry key is dropped", !SidekiqEnvelope.retries?(env("{}")), "")
check("retry: 0 takes the rules", SidekiqEnvelope.retries?(env("{\"retry\":0}")), "")
check("first failure is count 0", SidekiqEnvelope.next_retry_count(env("{\"retry\":true}")) == 0, "")
check("then one more", SidekiqEnvelope.next_retry_count(env("{\"retry\":true,\"retry_count\":4}")) == 5, "")
check("retry: 0 is exhausted at once", SidekiqEnvelope.exhausted?(env("{\"retry\":0}"), 0, 25, NOW_MS), "")
check("retry: 2 retries twice", !SidekiqEnvelope.exhausted?(env("{\"retry\":2}"), 1, 25, NOW_MS) &&
  SidekiqEnvelope.exhausted?(env("{\"retry\":2}"), 2, 25, NOW_MS), "")
check("retry: true uses max_retries", !SidekiqEnvelope.exhausted?(env("{\"retry\":true}"), 24, 25, NOW_MS) &&
  SidekiqEnvelope.exhausted?(env("{\"retry\":true}"), 25, 25, NOW_MS) &&
  SidekiqEnvelope.exhausted?(env("{\"retry\":true}"), 3, 3, NOW_MS), "")
check("retry_for counts from failed_at", !SidekiqEnvelope.exhausted?(env("{\"retry\":true,\"retry_for\":60,\"failed_at\":" + (NOW_MS - 30_000).to_s + "}"), 9, 3, NOW_MS) &&
  SidekiqEnvelope.exhausted?(env("{\"retry\":true,\"retry_for\":60,\"failed_at\":" + (NOW_MS - 61_000).to_s + "}"), 0, 25, NOW_MS), "")
check("retry_for with Float seconds", SidekiqEnvelope.exhausted?(env("{\"retry\":true,\"retry_for\":60,\"failed_at\":" + ((NOW_MS - 61_000) / 1000.0).to_s + "}"), 0, 25, NOW_MS), "")
check("retry_for on a first failure", !SidekiqEnvelope.exhausted?(env("{\"retry\":true,\"retry_for\":1}"), 0, 0, NOW_MS), "")
check("dead: false discards when exhausted", SidekiqEnvelope.discard_when_exhausted?(env("{\"retry\":true,\"dead\":false}")) &&
  !SidekiqEnvelope.discard_when_exhausted?(env("{\"retry\":true}")), "")
delays_ok = true
[0, 1, 4, 24].each do |count|
  20.times do
    d = SidekiqEnvelope.retry_delay(count)
    delays_ok = false if d < count**4 + 15 || d >= count**4 + 15 + 10 * (count + 1)
  end
end
check("backoff is count**4 + 15 plus jitter", delays_ok, "")

# The rewrite: a long message is cut, and retry_queue wins.
long = "x" * 10_005
cut = JSON.parse(SidekiqEnvelope.with_error("{\"retry\":true,\"queue\":\"a\"}", "a", "E", long, NOW_MS))
check("message cut to 10,000", cut["error_message"].length == 10_000, cut["error_message"].length.to_s)
rq = JSON.parse(SidekiqEnvelope.with_error("{\"retry\":true,\"queue\":\"a\",\"retry_queue\":\"slow\"}", "a", "E", "m", NOW_MS))
check("retry_queue wins", rq["queue"] == "slow", rq["queue"].to_s)

# The in-process drain is unchanged: a payload that raises is reported
# by the drain and not counted, one whose record is gone is reported,
# and a quiet discard says nothing.
ActiveJob.enqueue_payload(ActiveJob::Payload.build("BoomJob", "default",
  ActiveJob::Arguments.list([ActiveJob::Arguments.str("drained")])))
ActiveJob.enqueue_payload(ActiveJob::Payload.build("GoneQuietJob", "default", ActiveJob::Arguments.list([])))
ActiveJob.enqueue_payload(ActiveJob::Payload.build("RecordJob", "default",
  ActiveJob::Arguments.list([ActiveJob::Arguments.str("y")])))
check("drain counts the jobs that did not raise", ActiveJob.drain == 2, "")
puts "done"
