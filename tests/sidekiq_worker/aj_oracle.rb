#!/usr/bin/env ruby
# The oracle for `retry_on`, `discard_on` and `sidekiq_options` on
# ActiveJob classes under Sidekiq: what ActiveJob 8.1 and Sidekiq's
# adapter write to Redis when a job raises, run by Sidekiq's own
# wrapper (`Sidekiq::ActiveJob::Wrapper#perform`), as a server runs it.
#
#   ruby tests/sidekiq_worker/aj_oracle.rb > tests/sidekiq_worker/activejob.json
#
# Needs activejob 8.1.3 and sidekiq 8.1.7 installed, and redis-server
# and redis-cli on PATH. No Rails app is loaded, so `retry_jitter`
# defaults to 0.0 as it does before `load_defaults "6.1"`.
#
# Time.now is frozen and Kernel.rand answers 0.5, so every delay,
# score and timestamp is exact. Each scenario pushes a job, then runs
# what was written (the next `schedule` or queue entry) through the
# wrapper until the job stops re-enqueueing itself, and records each
# run's outcome and write.
require "json"
require "securerandom"
require_relative "oracle_support"

gem "activejob", "8.1.3"
gem "activesupport", "8.1.3"
gem "sidekiq", "8.1.7"
require "active_job"
require "sidekiq"
require File.join(Gem.loaded_specs["sidekiq"].full_gem_path,
  "lib/active_job/queue_adapters/sidekiq_adapter")

PORT = 16542
URL = "redis://127.0.0.1:#{PORT}/0"

system("redis-server", "--port", PORT.to_s, "--save", "", "--appendonly", "no",
  "--daemonize", "yes", "--pidfile", "/tmp/sidekiq-aj-oracle.pid",
  "--logfile", "/tmp/sidekiq-aj-oracle.log", exception: true)
50.times do
  break if system("redis-cli", "-p", PORT.to_s, "ping", out: File::NULL, err: File::NULL)
  sleep 0.1
end
at_exit { system("redis-cli", "-p", PORT.to_s, "shutdown", "nosave", out: File::NULL, err: File::NULL) }

Sidekiq.configure_client do |config|
  config.redis = { url: URL }
  config.logger.level = Logger::WARN
end
ActiveJob::Base.logger = Logger.new(File::NULL)
ActiveJob::Base.queue_adapter = :sidekiq

def redis(*argv) = Sidekiq.redis { |c| c.call(*argv) }

# The frozen clock. `tick` moves it, as time passes between runs.
START = Time.at(1_792_632_730, 250_000_000, :nsec).utc
NOW = [START]
class << Time
  def now = NOW[0].dup
end
def tick(seconds) = NOW[0] += seconds

module Kernel
  def self.rand(*args) = args.empty? ? 0.5 : super
end

class Sticky < StandardError; end
class Slippery < StandardError; end
class Gone < StandardError; end

class ApplicationJob < ActiveJob::Base
  discard_on ActiveJob::DeserializationError
end

class FixedWaitJob < ApplicationJob
  retry_on Sticky, wait: 1, attempts: 3
  def perform(name) = raise(Sticky, "#{name} sticks")
end

class PolyJob < ApplicationJob
  retry_on Sticky, Slippery, wait: :polynomially_longer, attempts: 4
  def perform(name) = raise(Slippery, "#{name} slips")
end

class DurationJob < ApplicationJob
  retry_on Sticky, wait: 5.minutes, attempts: :unlimited, queue: :low, priority: 5
  def perform(name) = raise(Sticky, "#{name} sticks")
end

class ClassJitterJob < ApplicationJob
  self.retry_jitter = 0.15
  retry_on Sticky, wait: 10, attempts: 2
  retry_on Slippery, wait: :polynomially_longer, attempts: 2
  def perform(name) = raise(name == "slip" ? Slippery : Sticky, name)
end

class HandlerJitterJob < ApplicationJob
  self.retry_jitter = 0.15
  retry_on Sticky, wait: 10, jitter: 0.5, attempts: 2
  def perform(name) = raise(Sticky, name)
end

class DiscardJob < ApplicationJob
  discard_on Gone
  def perform(name) = raise(Gone, "#{name} is gone")
end

# Searched last-declared first: the discard wins over the retry.
class OrderJob < ApplicationJob
  retry_on Sticky, wait: 1
  discard_on Sticky
  def perform(name) = raise(Sticky, name)
end

# Two ActiveJob tries, then Sidekiq's own retry rules take over.
class OptionsJob < ApplicationJob
  sidekiq_options retry: 3, retry_queue: "low"
  retry_on Sticky, wait: 1, attempts: 2
  def perform(name) = raise(Sticky, name)
end

class NoHandlerJob < ActiveJob::Base
  def perform(name) = raise(Sticky, name)
end

class ArgJob < ApplicationJob
  def perform(arg) = nil
end

class ArgNoHandlerJob < ActiveJob::Base
  def perform(arg) = nil
end

# The next entry the adapter wrote, removed from Redis: [set or queue,
# score or nil, raw]. ZRANGE WITHSCORES answers [member, score] pairs.
def take_write
  sched = redis("ZRANGE", "schedule", 0, -1, "WITHSCORES")
  queues = redis("KEYS", "queue:*").flat_map { |q| redis("LRANGE", q, 0, -1).map { |raw| [q, nil, raw] } }
  writes = sched.map { |raw, score| ["schedule", score, raw] } + queues
  raise "expected at most one write, got #{writes.size}" if writes.size > 1
  redis("FLUSHDB")
  writes.first
end

JID = /\A\h{24}\z/
UUID = /\A\h{8}-\h{4}-4\h{3}-[89ab]\h{3}-\h{12}\z/

def describe_write(write, previous_jid)
  where, score, raw = write
  env = JSON.parse(raw)
  raise "jid shape" unless env["jid"].match?(JID)
  raise "created_at shape" unless ms?(env["created_at"])
  env["jid"] = "<jid>"
  env["created_at"] = "<ms>"
  env["enqueued_at"] = "<ms>" if env.key?("enqueued_at")
  aj = env["args"][0]
  raise "job_id shape" unless aj["job_id"].match?(UUID)
  aj["job_id"] = "<uuid>"
  pj = aj["provider_job_id"]
  aj["provider_job_id"] = "<previous jid>" if pj && pj == previous_jid
  out = { "to" => where }
  if score
    out["score_minus_now"] = (score - NOW[0].to_f).round(6)
  end
  out.merge("envelope_keys" => top_level_keys(raw), "envelope" => env)
end

# Runs the job's envelope through Sidekiq's wrapper, as a server does.
def run(raw)
  env = JSON.parse(raw)
  wrapper = Sidekiq::ActiveJob::Wrapper.new
  wrapper.jid = env["jid"]
  wrapper.perform(env["args"][0])
  nil
rescue => e
  "#{e.class}: #{e.message}"
end

def scenario(max_runs = 6)
  redis("FLUSHDB")
  yield
  first = take_write
  steps = [{ "pushed" => describe_write(first, nil) }]
  raw = first[2]
  max_runs.times do |i|
    tick(60)
    raised = run(raw)
    write = take_write
    step = { "run" => i + 1, "raised" => raised }
    step["wrote"] = describe_write(write, JSON.parse(raw)["jid"]) if write
    steps << step
    break unless write
    raw = write[2]
  end
  steps
end

def bogus(job_class)
  { "job_class" => job_class.name, "job_id" => SecureRandom.uuid, "provider_job_id" => nil,
    "queue_name" => "default", "priority" => nil,
    "arguments" => [{ "_aj_serialized" => "NoSuchSerializer" }],
    "executions" => 0, "exception_executions" => {}, "locale" => "en", "timezone" => nil,
    "enqueued_at" => Time.now.utc.iso8601(9), "scheduled_at" => nil }
end

def push_raw(job_class, aj)
  Sidekiq::Client.push("class" => Sidekiq::ActiveJob::Wrapper, "wrapped" => job_class.name,
    "queue" => "default", "args" => [aj])
end

results = {}
results["fixed_wait"] = scenario { FixedWaitJob.perform_later("fixed") }
results["polynomially_longer_two_exceptions"] = scenario { PolyJob.perform_later("poly") }
results["duration_unlimited_queue_priority"] = scenario(3) { DurationJob.perform_later("dur") }
results["class_jitter_fixed"] = scenario { ClassJitterJob.perform_later("stick") }
results["class_jitter_polynomially_longer"] = scenario { ClassJitterJob.perform_later("slip") }
results["handler_jitter"] = scenario { HandlerJitterJob.perform_later("hj") }
results["discard_on"] = scenario { DiscardJob.perform_later("gone") }
results["search_order"] = scenario { OrderJob.perform_later("order") }
results["sidekiq_options_then_exhausted"] = scenario { OptionsJob.perform_later("opts") }
results["no_handler"] = scenario { NoHandlerJob.perform_later("none") }
results["deserialization_discarded"] = scenario { push_raw(ArgJob, bogus(ArgJob)) }
results["deserialization_no_handler"] = scenario { push_raw(ArgNoHandlerJob, bogus(ArgNoHandlerJob)) }

# Sidekiq retrying an exhausted OptionsJob: the text it retries is the
# last one ActiveJob scheduled, so each run counts to the limit again and
# raises at once, with no write.
redis("FLUSHDB")
OptionsJob.perform_later("again")
raw = take_write[2]
reruns = []
4.times do
  tick(60)
  raised = run(raw)
  write = take_write
  reruns << { "raised" => raised, "wrote" => !write.nil? }
  raw = write[2] if write
end

puts JSON.pretty_generate(
  "versions" => {
    "sidekiq" => Sidekiq::VERSION,
    "activejob" => ActiveJob.version.to_s,
    "redis_server" => redis("INFO", "server")[/redis_version:(\S+)/, 1]
  },
  "clock_starts_at" => START.iso8601(9),
  "clock_ticks_before_each_run" => 60,
  "kernel_rand" => 0.5,
  "default_retry_jitter" => ActiveJob::Base.retry_jitter,
  "scenarios" => results,
  "exhausted_payload_reruns" => reruns
)
