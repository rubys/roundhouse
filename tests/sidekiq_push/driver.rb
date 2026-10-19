# Sidekiq pushes, written by the runtime a tree on queue_adapter
# :sidekiq runs: runtime/spinel/sidekiq_push.rb over the payload writer
# in runtime/spinel/active_job_serialization.rb.
#
# tests/sidekiq_push.rs copies this file beside the runtime sources it
# requires, then runs it under CRuby and, compiled by spinel, as a
# binary. Pushes go to the recording seam, so no Redis is involved. It
# enqueues oracle.rb's jobs, in the same order, and prints one line per
# command for the harness to hold against Sidekiq's own (sidekiq.json):
#
#   push <name>
#   cmd <verb> <key> <score or -> <json or ->
#
# then the after-commit hold's checks, one `ok`/`FAIL` line each.
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

def check(name, ok, detail)
  puts (ok ? "ok   " : "FAIL ") + name + (ok ? "" : " (" + detail + ")")
end

def record(gid)
  ActiveJob::Arguments.list([ActiveJob::Arguments.record(gid)])
end

def show(name)
  puts "push " + name
  SidekiqPush.recorded.each do |line|
    parts = line.split(" ", 3)
    verb = parts[0]
    key = parts[1]
    rest = parts[2]
    if verb == "zadd"
      sp = rest.index(" ")
      puts "cmd zadd " + key + " " + rest[0, sp] + " " + rest[sp + 1, rest.length - sp - 1]
    elsif verb == "lpush"
      puts "cmd lpush " + key + " - " + rest
    else
      puts "cmd " + verb + " " + key + " - " + rest
    end
  end
  SidekiqPush.start_recording
  nil
end

def push(job_class, queue, at, args_json)
  SidekiqPush.enqueue(job_class, queue, at,
    ActiveJob::Payload.build_at(job_class, queue, at, args_json))
end

SidekiqPush.start_recording

push("InspectCarJob", "default", 0.0, record("gid://depot/Car/1"))
show("immediate on default")
push("WashCarJob", "low", 0.0, record("gid://depot/Car/2"))
show("queue_as low")
push("InspectCarJob", "critical", 0.0, record("gid://depot/Car/3"))
show("set(queue: :critical)")
push("InspectCarJob", "default", Time.now.to_f + 300.0, record("gid://depot/Car/4"))
show("set(wait: 300)")
push("WashCarJob", "low", Time.now.to_f + 5 * 60.0, record("gid://depot/Car/5"))
show("set(wait: 5.minutes) on low")
push("InspectCarJob", "default", Time.now.to_f - 60.0, record("gid://depot/Car/6"))
show("set(wait_until:) in the past")
SidekiqPush.enqueue_after_commit("AfterCommitJob", "default", 0.0,
  ActiveJob::Payload.build_at("AfterCommitJob", "default", 0.0, record("gid://depot/Car/7")))
show("enqueue_after_transaction_commit, no transaction")

a = ActiveJob::Arguments
t = Time.at(1_791_548_096, 123_456_789, :nsec).utc
every = a.list([
  a.null, a.bool(true), a.bool(false),
  a.int(42), a.int(-7), a.int(2**40),
  a.float(1.5), a.float(0.1), a.float(-2.5e-08), a.float(1.0e+20),
  a.str("plain"), a.str("quote\" backslash\\ nl\n tab\t ctrl\u0001 <&> é ✓  /  "), a.str(""),
  a.sym(:open),
  a.record("gid://depot/Car/7"), a.record("gid://depot/Cars::Freight/8"),
  a.time(t),
  a.list([a.int(1), a.int(2), a.int(3)]), a.list([a.str("a"), a.str("b")]), a.list([]),
  a.list([a.bool(true), a.bool(false)]), a.list([a.sym(:x), a.sym(:y)])
])
push("EveryArgumentJob", "default", 0.0, every)
show("every Phase 1 argument type")

# The after-commit hold: nothing is pushed while the transaction is
# open, everything at COMMIT in order, nothing after a ROLLBACK.
ActiveJob.transaction_began
SidekiqPush.enqueue_after_commit("AfterCommitJob", "default", 0.0,
  ActiveJob::Payload.build_at("AfterCommitJob", "default", 0.0, record("gid://depot/Car/8")))
SidekiqPush.enqueue_after_commit("AfterCommitJob", "low", Time.now.to_f + 60.0,
  ActiveJob::Payload.build_at("AfterCommitJob", "low", Time.now.to_f + 60.0, record("gid://depot/Car/9")))
check("held while the transaction is open", SidekiqPush.recorded.length == 0, SidekiqPush.recorded.length.to_s)
ActiveJob.transaction_committed
got = SidekiqPush.recorded
check("pushed at COMMIT, in order", got.length == 3 && got[0] == "sadd queues default" && got[2].start_with?("zadd schedule "), got.length.to_s)
check("the hold is empty after COMMIT", SidekiqPush.held.length == 0, SidekiqPush.held.length.to_s)
SidekiqPush.start_recording

ActiveJob.transaction_began
SidekiqPush.enqueue_after_commit("AfterCommitJob", "default", 0.0,
  ActiveJob::Payload.build_at("AfterCommitJob", "default", 0.0, record("gid://depot/Car/10")))
ActiveJob.transaction_rolled_back
check("dropped at ROLLBACK", SidekiqPush.recorded.length == 0 && SidekiqPush.held.length == 0, SidekiqPush.recorded.length.to_s)

# The URL, as Sidekiq finds it, and its redaction.
check("redacted password", SidekiqPush.redacted_url("redis://app:hunter2@cache:6379/2") == "redis://app:REDACTED@cache:6379/2", SidekiqPush.redacted_url("redis://app:hunter2@cache:6379/2"))
check("redacted bare password", SidekiqPush.redacted_url("redis://:hunter2@cache") == "redis://:REDACTED@cache", SidekiqPush.redacted_url("redis://:hunter2@cache"))
check("no password to redact", SidekiqPush.redacted_url("redis://cache:6379/0") == "redis://cache:6379/0", SidekiqPush.redacted_url("redis://cache:6379/0"))

# configure_client's settings, read from the environment. An app that
# gives none (this registry's `""`) falls back to REDIS_URL.
ENV["SK_TEST_SET"] = "redis://set:6379/1"
ENV.delete("SK_TEST_UNSET")
check("ENV[] setting", SidekiqPush.setting("SK_TEST_SET") == "redis://set:6379/1", SidekiqPush.setting("SK_TEST_SET"))
check("ENV[] setting, unset", SidekiqPush.setting("SK_TEST_UNSET") == "", SidekiqPush.setting("SK_TEST_UNSET"))
check("ENV.fetch default", SidekiqPush.setting_or("SK_TEST_UNSET", "redis://dflt:6379/2") == "redis://dflt:6379/2", SidekiqPush.setting_or("SK_TEST_UNSET", "redis://dflt:6379/2"))
check("ENV.fetch, set", SidekiqPush.setting_or("SK_TEST_SET", "redis://dflt:6379/2") == "redis://set:6379/1", SidekiqPush.setting_or("SK_TEST_SET", "redis://dflt:6379/2"))
raised = false
begin
  SidekiqPush.required_setting("SK_TEST_UNSET")
rescue KeyError
  raised = true
end
check("ENV.fetch without default raises", raised, "no KeyError")
ENV["REDIS_PROVIDER"] = ""
ENV["REDIS_URL"] = "redis://fallback:6379/3"
check("no configure_client url: falls back", SidekiqPush.resolve_url == "redis://fallback:6379/3", SidekiqPush.resolve_url)
check("no configure_client password", SidekiqPush.password == "", SidekiqPush.password)

SidekiqPush.stop_recording
puts "done"
