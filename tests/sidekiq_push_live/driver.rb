# Sidekiq pushes through a real Redis client: the runtime a tree on
# queue_adapter :sidekiq runs (runtime/spinel/sidekiq_push.rb over
# runtime/spinel/sidekiq/sidekiq_redis.rb), with redis-rb under CRuby and
# spinel-redis when spinel compiles this file.
#
# tests/sidekiq_push_live.rs copies it beside the runtime sources and
# runs it with REDIS_URL naming a scratch database; reader.rb then reads
# what landed there through Sidekiq's own API. It pushes an immediate job
# on each of two queues, a scheduled job, an after-commit job held to
# COMMIT, and one held then dropped at ROLLBACK.
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

def record(gid)
  ActiveJob::Arguments.list([ActiveJob::Arguments.record(gid)])
end

def push(job_class, queue, at, gid)
  SidekiqPush.enqueue(job_class, queue, at, ActiveJob::Payload.build_at(job_class, queue, at, record(gid)))
end

push("InspectCarJob", "default", 0.0, "gid://depot/Car/1")
push("WashCarJob", "low", 0.0, "gid://depot/Car/2")
push("InspectCarJob", "default", Time.now.to_f + 300.0, "gid://depot/Car/3")

ActiveJob.transaction_began
SidekiqPush.enqueue_after_commit("AfterCommitJob", "default", 0.0,
  ActiveJob::Payload.build_at("AfterCommitJob", "default", 0.0, record("gid://depot/Car/4")))
ActiveJob.transaction_committed

ActiveJob.transaction_began
SidekiqPush.enqueue_after_commit("AfterCommitJob", "default", 0.0,
  ActiveJob::Payload.build_at("AfterCommitJob", "default", 0.0, record("gid://depot/Car/5")))
ActiveJob.transaction_rolled_back

puts "pushed to " + SidekiqPush.redacted_url(SidekiqPush.url)
