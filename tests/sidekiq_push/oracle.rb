#!/usr/bin/env ruby
# The oracle for the Sidekiq push golden: what real Sidekiq 8.1's
# ActiveJob adapter sends to Redis for a fixed set of jobs.
#
#   ruby tests/sidekiq_push/oracle.rb > tests/sidekiq_push/sidekiq.json
#
# Needs activejob 8.1.3, globalid, sidekiq 8.1.7 (with redis-client and
# connection_pool) installed, and redis-server and redis-cli on PATH.
# It starts its own redis-server on a private port.
#
# Every command the client sends is recorded through a redis-client
# middleware, with the pipeline it went out in. Values that change per
# run are checked for shape and then masked: the jid, the envelope's
# millisecond timestamps, ActiveJob's job_id and enqueued_at, and
# scheduled_at. A ZADD score is written as its offset from created_at,
# in seconds. Each envelope's keys are also recorded in wire order,
# from the raw JSON text.
gem "activejob", "8.1.3"
gem "activesupport", "8.1.3"
gem "sidekiq", "8.1.7"
require "active_job"
require "active_support/core_ext/time"
require "global_id"
require "json"
require "sidekiq"
require File.join(Gem.loaded_specs["sidekiq"].full_gem_path,
  "lib/active_job/queue_adapters/sidekiq_adapter")

PORT = 16530

system("redis-server", "--port", PORT.to_s, "--save", "", "--appendonly", "no",
  "--daemonize", "yes", "--pidfile", "/tmp/sidekiq-push-oracle.pid",
  "--logfile", "/tmp/sidekiq-push-oracle.log", exception: true)
50.times do
  break if system("redis-cli", "-p", PORT.to_s, "ping", out: File::NULL, err: File::NULL)
  sleep 0.1
end
at_exit { system("redis-cli", "-p", PORT.to_s, "shutdown", "nosave", out: File::NULL, err: File::NULL) }

# Each recorded entry is one round trip: a single command, or a pipeline.
module Recorder
  LOG = []

  def call(command, redis_config)
    LOG << { "pipelined" => false, "commands" => [command.map(&:to_s)] }
    super
  end

  def call_pipelined(commands, redis_config)
    LOG << { "pipelined" => true, "commands" => commands.map { |c| c.map(&:to_s) } }
    super
  end
end
RedisClient.register(Recorder)

Sidekiq.configure_client do |config|
  config.redis = { url: "redis://127.0.0.1:#{PORT}/0" }
  config.logger.level = Logger::WARN
end
Sidekiq.redis { |c| c.call("FLUSHDB") }

GlobalID.app = "depot"
Time.zone = "UTC"
ActiveJob::Base.logger = Logger.new(File::NULL)
ActiveJob::Base.queue_adapter = :sidekiq

class Car
  include GlobalID::Identification
  attr_reader :id
  def initialize(id) = @id = id
end

module Cars
  class Freight < Car; end
end

class InspectCarJob < ActiveJob::Base
  queue_as :default
  def perform(car); end
end

class WashCarJob < ActiveJob::Base
  queue_as :low
  def perform(car); end
end

class AfterCommitJob < ActiveJob::Base
  self.enqueue_after_transaction_commit = true
  def perform(car); end
end

class EveryArgumentJob < ActiveJob::Base
  def perform(*args); end
end

t = Time.at(1_791_548_096, 123_456_789, :nsec).utc
every_argument = [
  nil, true, false,
  42, -7, 2**40,
  1.5, 0.1, -2.5e-08, 1.0e+20,
  "plain", "quote\" backslash\\ nl\n tab\t ctrl\u0001 <&> é ✓  / \u2028", "",
  :open,
  Car.new(7), Cars::Freight.new(8),
  t,
  [1, 2, 3], ["a", "b"], [], [true, false], [:x, :y]
]

def mask_envelope(raw)
  env = JSON.parse(raw)
  created = env["created_at"]
  raise "created_at is not integer ms: #{created.inspect}" unless created.is_a?(Integer) && created.to_s.length == 13
  raise "jid shape: #{env["jid"]}" unless env["jid"].match?(/\A[0-9a-f]{24}\z/)
  env["jid"] = "<jid>"
  env["created_at"] = "<ms>"
  if env.key?("enqueued_at")
    raise "enqueued_at is not integer ms" unless env["enqueued_at"].is_a?(Integer)
    env["enqueued_at"] = "<ms>"
  end
  env["args"] = env["args"].map do |aj|
    aj = aj.dup
    raise "job_id shape" unless aj["job_id"].match?(/\A\h{8}-\h{4}-4\h{3}-[89ab]\h{3}-\h{12}\z/)
    aj["job_id"] = "<uuid>"
    raise "enqueued_at shape" unless aj["enqueued_at"].match?(/\A\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{9}Z\z/)
    aj["enqueued_at"] = "<iso8601(9)>"
    if aj["scheduled_at"]
      raise "scheduled_at shape" unless aj["scheduled_at"].match?(/\A\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{9}Z\z/)
      aj["scheduled_at"] = "<iso8601(9)>"
    end
    aj
  end
  [env, created]
end

# The envelope's keys in the order they appear in the JSON text, at the
# top level only: a small scan that tracks nesting and strings.
def top_level_keys(raw)
  keys = []
  depth = 0
  i = 0
  while i < raw.length
    c = raw[i]
    if c == '"'
      j = i + 1
      j += (raw[j] == "\\" ? 2 : 1) while raw[j] != '"'
      keys << JSON.parse(raw[i..j]) if depth == 1 && raw[j + 1] == ":"
      i = j
    elsif c == "{" || c == "["
      depth += 1
    elsif c == "}" || c == "]"
      depth -= 1
    end
    i += 1
  end
  keys
end

def record(name)
  Recorder::LOG.clear
  yield
  trips = Recorder::LOG.map do |trip|
    commands = trip["commands"].map do |argv|
      verb = argv[0].downcase
      case verb
      when "lpush"
        env, = mask_envelope(argv[2])
        { "argv" => [verb, argv[1]], "envelope_keys" => top_level_keys(argv[2]), "envelope" => env }
      when "zadd"
        env, created = mask_envelope(argv[3])
        offset = (argv[2].to_f - created / 1000.0).round(1)
        { "argv" => [verb, argv[1]], "score_minus_created_at" => offset,
          "score_is_float_to_s" => argv[2] == argv[2].to_f.to_s,
          "envelope_keys" => top_level_keys(argv[3]), "envelope" => env }
      else
        { "argv" => [verb] + argv[1..] }
      end
    end
    { "pipelined" => trip["pipelined"], "commands" => commands }
  end
  { "name" => name, "round_trips" => trips }
end

pushes = []
pushes << record("immediate on default") { InspectCarJob.perform_later(Car.new(1)) }
pushes << record("queue_as low") { WashCarJob.perform_later(Car.new(2)) }
pushes << record("set(queue: :critical)") { InspectCarJob.set(queue: :critical).perform_later(Car.new(3)) }
pushes << record("set(wait: 300)") { InspectCarJob.set(wait: 300).perform_later(Car.new(4)) }
pushes << record("set(wait: 5.minutes) on low") { WashCarJob.set(wait: 5.minutes).perform_later(Car.new(5)) }
pushes << record("set(wait_until:) in the past") { InspectCarJob.set(wait_until: Time.now - 60).perform_later(Car.new(6)) }
pushes << record("enqueue_after_transaction_commit, no transaction") { AfterCommitJob.perform_later(Car.new(7)) }
pushes << record("every Phase 1 argument type") { EveryArgumentJob.perform_later(*every_argument) }

# What Redis holds afterwards, as Sidekiq's own API would read it.
state = Sidekiq.redis do |c|
  {
    "queues" => c.call("SMEMBERS", "queues").sort,
    "lengths" => c.call("SMEMBERS", "queues").sort.to_h { |q| [q, c.call("LLEN", "queue:#{q}")] },
    "schedule_size" => c.call("ZCARD", "schedule")
  }
end

puts JSON.pretty_generate(
  "sidekiq" => Sidekiq::VERSION,
  "activejob" => ActiveJob.version.to_s,
  "redis_client" => RedisClient::VERSION,
  "json" => JSON::VERSION,
  "adapter" => ActiveJob::Base.queue_adapter.class.name,
  "pushes" => pushes,
  "redis_after" => state
)
