# Loaded into the real Sidekiq server by tests/sidekiq_worker/
# retry_oracle.rb (`sidekiq -r`). Defines the jobs that fail, and
# records every Redis command the server sends, with the reply to each
# single command, one JSON line per round trip, to $ORACLE_LOG.
gem "activejob", "8.1.3"
gem "activesupport", "8.1.3"
gem "sidekiq", "8.1.7"
require "active_job"
require "json"
require "sidekiq"
require File.join(Gem.loaded_specs["sidekiq"].full_gem_path,
  "lib/active_job/queue_adapters/sidekiq_adapter")

module RetryOracleLog
  LOCK = Mutex.new
  FILE = File.open(ENV.fetch("ORACLE_LOG"), "a")
  FILE.sync = true

  def self.write(entry)
    LOCK.synchronize { FILE.puts(JSON.generate(entry)) }
  end
end

module RetryRecorder
  def call(command, redis_config)
    argv = command.map(&:to_s)
    begin
      reply = super
    rescue => e
      RetryOracleLog.write("t" => Time.now.to_f, "kind" => "call", "commands" => [argv], "error" => e.message)
      raise
    end
    RetryOracleLog.write("t" => Time.now.to_f, "kind" => "call", "commands" => [argv],
      "reply" => reply.is_a?(Float) ? reply.to_s : reply)
    reply
  end

  def call_pipelined(commands, redis_config)
    kind = commands.first&.first.to_s.casecmp?("multi") ? "multi" : "pipelined"
    RetryOracleLog.write("t" => Time.now.to_f, "kind" => kind, "commands" => commands.map { |c| c.map(&:to_s) })
    super
  end
end
RedisClient.register(RetryRecorder)

ActiveJob::Base.logger = Logger.new(File::NULL)
ActiveJob::Base.queue_adapter = :sidekiq

# Every job notes each attempt in oracle:attempts ("<name> <n>"), and a
# run that succeeds in oracle:ran, so the oracle can wait on them.
module Attempts
  def self.note(name)
    n = Sidekiq.redis { |c| c.call("INCR", "oracle:attempt:#{name}") }
    Sidekiq.redis { |c| c.call("RPUSH", "oracle:attempts", "#{name} #{n}") }
    n
  end

  def self.ran(name)
    Sidekiq.redis { |c| c.call("RPUSH", "oracle:ran", name) }
  end
end

class Sticky < StandardError; end

# Raises on its first `fails` runs, then succeeds.
class FlakyJob < ActiveJob::Base
  def perform(name, fails)
    n = Attempts.note(name)
    raise Sticky, "#{name} attempt #{n}" if n <= fails
    Attempts.ran(name)
  end
end

class AlwaysJob < ActiveJob::Base
  def perform(name)
    raise Sticky, "#{name} attempt #{Attempts.note(name)}"
  end
end

class TwoRetriesJob < AlwaysJob
  sidekiq_options retry: 2
end

class NoRetryJob < AlwaysJob
  sidekiq_options retry: false
end

class ZeroRetryJob < AlwaysJob
  sidekiq_options retry: 0
end

class RetryForJob < AlwaysJob
  sidekiq_options retry_for: 1
end

class NotDeadJob < AlwaysJob
  sidekiq_options retry: 1, dead: false
end

class LowRetryJob < FlakyJob
  sidekiq_options retry: 3, retry_queue: "low"
end

class RecordJob < ActiveJob::Base
  def perform(name)
    Attempts.ran(name)
  end
end
