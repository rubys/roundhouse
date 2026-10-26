# The worker role against a real Redis: the runtime a tree on
# queue_adapter :sidekiq runs as `./app sidekiq` (runtime/spinel/sidekiq/
# sidekiq_worker.rb over runtime/spinel/sidekiq/sidekiq_redis.rb), with
# redis-rb under CRuby and spinel-redis when spinel compiles this file.
#
# tests/sidekiq_worker_live.rs copies it beside the runtime sources and
# runs it with REDIS_URL naming a scratch database. It pushes jobs the
# way the web role does, runs two processor threads until they have
# taken them all, then shuts down with a slow job still running, as
# TERM does. reader.rb then reads what is left through Sidekiq's own
# API: the dead set, and the slow job back on its queue.
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

module Db
  def self.with_connection
    result = yield
    result
  end
end

# The worker clears the broadcast log (runtime/spinel/broadcasts.rb)
# before each job; these jobs broadcast nothing.
module Broadcasts
  def self.reset_log!
    nil
  end
end

require_relative "active_job"
require_relative "global_id_locator"
require_relative "active_job_serialization"
require_relative "sidekiq_redis"
require_relative "sidekiq_push"
require_relative "job_registry"
require_relative "sidekiq_envelope"
require_relative "sidekiq_worker"

module JobRegistry
  RAN = [""]
  RAN_LOCK = Mutex.new

  def self.perform(job_class, args)
    if job_class == "BoomJob"
      raise ArgumentError, "boom " + ActiveJob::Arguments.str_at(args, 0)
    elsif job_class == "SlowJob"
      RAN_LOCK.synchronize { RAN.push("slow started") }
      sleep 5
      true
    elsif job_class == "RecordJob"
      RAN_LOCK.synchronize { RAN.push(ActiveJob::Arguments.str_at(args, 0)) }
      true
    else
      false
    end
  end

  def self.discards_deserialization_error(job_class)
    false
  end
end
JobRegistry::RAN.clear

def push(job_class, queue, label)
  SidekiqPush.enqueue(job_class, queue, 0.0, ActiveJob::Payload.build_at(job_class, queue, 0.0,
    ActiveJob::Arguments.list([ActiveJob::Arguments.str(label)])))
end

def ran
  JobRegistry::RAN_LOCK.synchronize { JobRegistry::RAN.dup }
end

def wait_until(what, seconds)
  waited = 0.0
  while !yield
    raise "timed out waiting for " + what if waited > seconds
    sleep 0.05
    waited += 0.05
  end
  nil
end

push("RecordJob", "default", "r1")
push("RecordJob", "low", "r2")
push("BoomJob", "default", "b1")
push("MailerJob", "low", "m1")
push("RecordJob", "default", "r3")

probe = SidekiqRedis.worker_open(SidekiqPush.url)
SidekiqWorker.parse(["sidekiq", "-q", "default", "-q", "low", "-c", "2", "-t", "1"])
SidekiqWorker.install_traps
SidekiqWorker.start(2)
wait_until("the first jobs", 20.0) { ran.length == 3 && probe.zcard("dead") == 2 }
puts "ran " + ran.sort.join(",")

push("SlowJob", "default", "s1")
wait_until("the slow job", 20.0) { ran.include?("slow started") }
Process.kill("TERM", Process.pid)
SidekiqWorker.supervise
puts "requeued, " + probe.llen("queue:default").to_s + " on default"
probe.close
puts "worked " + SidekiqPush.redacted_url(SidekiqPush.url)
SidekiqRedis.hard_exit(0)
