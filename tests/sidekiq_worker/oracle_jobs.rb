# Loaded into the real Sidekiq server by tests/sidekiq_worker/oracle.rb
# (`sidekiq -r`). Defines the oracle's jobs and records every Redis
# command the server sends, one JSON line per round trip, to
# $ORACLE_LOG.
gem "activejob", "8.1.3"
gem "activesupport", "8.1.3"
gem "sidekiq", "8.1.7"
require "active_job"
require "json"
require "sidekiq"
require File.join(Gem.loaded_specs["sidekiq"].full_gem_path,
  "lib/active_job/queue_adapters/sidekiq_adapter")

module OracleLog
  LOCK = Mutex.new
  FILE = File.open(ENV.fetch("ORACLE_LOG"), "a")
  FILE.sync = true

  def self.write(kind, commands)
    LOCK.synchronize { FILE.puts(JSON.generate("kind" => kind, "commands" => commands)) }
  end
end

module Recorder
  def call(command, redis_config)
    OracleLog.write("call", [command.map(&:to_s)])
    super
  end

  def call_pipelined(commands, redis_config)
    kind = commands.first&.first.to_s.casecmp?("multi") ? "multi" : "pipelined"
    OracleLog.write(kind, commands.map { |c| c.map(&:to_s) })
    super
  end
end
RedisClient.register(Recorder)

ActiveJob::Base.logger = Logger.new(File::NULL)
ActiveJob::Base.queue_adapter = :sidekiq

# Each run appends its name to oracle:ran, so the oracle knows when the
# server has worked through the queue and in which order.
class RecordJob < ActiveJob::Base
  def perform(name)
    Sidekiq.redis { |c| c.call("RPUSH", "oracle:ran", name) }
  end
end

class BoomJob < ActiveJob::Base
  sidekiq_options retry: 0
  def perform(name)
    raise ArgumentError, "boom \"#{name}\"\nsecond line"
  end
end

class SlowJob < ActiveJob::Base
  def perform(name)
    Sidekiq.redis { |c| c.call("RPUSH", "oracle:ran", "#{name} started") }
    sleep 10
  end
end
