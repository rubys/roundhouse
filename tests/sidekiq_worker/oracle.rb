#!/usr/bin/env ruby
# The oracle for the Sidekiq worker: what a real Sidekiq 8.1 server
# sends to Redis while it fetches, runs, fails, and shuts down.
#
#   ruby tests/sidekiq_worker/oracle.rb > tests/sidekiq_worker/sidekiq.json
#
# Needs activejob 8.1.3, sidekiq 8.1.7 (with redis-client and
# connection_pool) installed, and redis-server and redis-cli on PATH.
# It starts its own redis-server on a private port and runs the real
# `sidekiq` executable against it, with tests/sidekiq_worker/
# oracle_jobs.rb loaded through `-r`. That file records every round
# trip the server makes.
#
# Three scenarios:
#   strict   - `-q high -q low`: two jobs that run, a job that raises
#              with `retry: 0`, malformed JSON, and a job on its last
#              retry that already carries Sidekiq's error fields
#   weighted - `-q a,2 -q b,1` over 200 fetches
#   shutdown - TSTP, then TERM with a job still running past `-t 1`
#
# Only the commands the worker itself must send are kept: fetches,
# dead-set writes and requeues. Heartbeat, stats and the jobs' own
# bookkeeping (oracle:*) are dropped. Values that change per run are
# checked for shape and then masked.
require "json"
require "rbconfig"

PORT = 16540
URL = "redis://127.0.0.1:#{PORT}/0"
DIR = File.expand_path(__dir__)
require "tmpdir"
require_relative "oracle_support"
WORK = Dir.mktmpdir("sidekiq-worker-oracle")

ENV["ORACLE_LOG"] = File.join(WORK, "client.log")
ENV["REDIS_URL"] = URL
require_relative "oracle_jobs"
Sidekiq.configure_client do |config|
  config.redis = { url: URL }
  config.logger.level = Logger::WARN
end

system("redis-server", "--port", PORT.to_s, "--save", "", "--appendonly", "no",
  "--daemonize", "yes", "--pidfile", "/tmp/sidekiq-worker-oracle.pid",
  "--logfile", "/tmp/sidekiq-worker-oracle.log", exception: true)
50.times do
  break if system("redis-cli", "-p", PORT.to_s, "ping", out: File::NULL, err: File::NULL)
  sleep 0.1
end
at_exit { system("redis-cli", "-p", PORT.to_s, "shutdown", "nosave", out: File::NULL, err: File::NULL) }

def redis(*argv) = Sidekiq.redis { |c| c.call(*argv) }

def wait_for(what, seconds = 30)
  deadline = Time.now + seconds
  until yield
    raise "timed out waiting for #{what}" if Time.now > deadline
    sleep 0.05
  end
end

# Runs the real server with `args`, yields its pid, and answers the
# round trips it logged, oldest first.
def server(name, *args)
  log = File.join(WORK, "#{name}.log")
  out = File.join(WORK, "#{name}.out")
  bin = Gem.bin_path("sidekiq", "sidekiq", "8.1.7")
  pid = Process.spawn({ "ORACLE_LOG" => log, "REDIS_URL" => URL },
    RbConfig.ruby, bin, "-r", File.join(DIR, "oracle_jobs.rb"), *args,
    out: out, err: out)
  begin
    wait_for("#{name} to boot") { File.exist?(log) && File.size(log) > 0 }
    yield pid
    _, status = Process.wait2(pid)
    raise "#{name} exited with #{status.inspect}\n#{File.read(out)}" unless status.success?
  rescue
    Process.kill("KILL", pid) rescue nil
    raise
  end
  File.readlines(log).map { |l| JSON.parse(l) }
end

KEPT = %w[brpop zadd zremrangebyscore zremrangebyrank rpush lpush multi exec].freeze

def kept(trips)
  trips.filter_map do |trip|
    commands = trip["commands"].reject do |argv|
      !KEPT.include?(argv[0].downcase) || argv[1].to_s.start_with?("oracle:")
    end
    next if commands.empty? || commands.map { |c| c[0].downcase } == %w[multi exec]
    { "kind" => trip["kind"], "commands" => commands }
  end
end

# One command, with its per-run values masked. `now` is the dead write's
# score, which the trim commands that follow it are relative to.
def describe(argv, now)
  verb = argv[0].downcase
  case verb
  when "zadd"
    score = argv[2]
    raise "dead score #{score} is not Float#to_s" unless score == score.to_f.to_s
    now[0] = score.to_f
    raw = argv[3]
    json = (JSON.parse(raw) rescue nil)
    entry = { "argv" => [verb, argv[1], "<now as Float#to_s>"] }
    if json
      entry.merge("envelope_keys" => top_level_keys(raw), "envelope" => mask_envelope(raw))
    else
      entry.merge("raw" => raw)
    end
  when "zremrangebyscore"
    offset = (now[0] - argv[3].to_f).round
    { "argv" => [verb, argv[1], argv[2], "<now - #{offset}>"] }
  when "rpush", "lpush"
    { "argv" => [verb, argv[1]], "values" => argv[2..].map { |v| top_level_keys(v) == [] ? v : "<envelope>" } }
  else
    { "argv" => [verb] + argv[1..] }
  end
end

# Consecutive identical round trips (the idle fetches before TERM, whose
# number depends on timing) are written once.
def describe_trips(trips)
  now = [0.0]
  described = trips.map do |trip|
    { "kind" => trip["kind"], "commands" => trip["commands"].map { |argv| describe(argv, now) } }
  end
  described.chunk_while { |a, b| a == b }.map(&:first)
end

versions = {
  "sidekiq" => Sidekiq::VERSION,
  "activejob" => ActiveJob.version.to_s,
  "redis_client" => RedisClient::VERSION,
  "redis_server" => redis("INFO", "server")[/redis_version:(\S+)/, 1]
}

# --- strict -----------------------------------------------------------
redis("FLUSHDB")
RecordJob.set(queue: :low).perform_later("l1")
RecordJob.set(queue: :high).perform_later("h1")
BoomJob.set(queue: :high).perform_later("b1")
MALFORMED = '{"class":"Sidekiq::ActiveJob::Wrapper","args":[not json'
redis("LPUSH", "queue:high", MALFORMED)
# A job Ruby Sidekiq has already retried three times, on its last try:
# the error fields are there, and the failure exhausts `retry: 3`.
BoomJob.set(queue: :staging).perform_later("r3")
retried = JSON.parse(redis("RPOP", "queue:staging"))
retried["retry"] = 3
retried["error_message"] = "old message"
retried["error_class"] = "OldError"
retried["failed_at"] = 1_000_000_000_000
retried["retry_count"] = 3
retried["retried_at"] = 1_000_000_010_000
redis("LPUSH", "queue:high", JSON.generate(retried))
pushed_high = redis("LRANGE", "queue:high", 0, -1).reverse.map { |raw| pin(raw) }
trips = server("strict", "-q", "high", "-q", "low", "-c", "1", "-t", "1") do |pid|
  wait_for("strict jobs") { redis("LLEN", "oracle:ran") == 2 && redis("ZCARD", "dead") == 3 }
  Process.kill("TERM", pid)
end
strict = kept(trips)
strict_ran = redis("LRANGE", "oracle:ran", 0, -1)
dead_after = redis("ZRANGE", "dead", 0, -1).map { |raw| (JSON.parse(raw) rescue nil) ? top_level_keys(raw) : raw }

# --- weighted ---------------------------------------------------------
redis("FLUSHDB")
120.times { |i| RecordJob.set(queue: :a).perform_later("a#{i}") }
120.times { |i| RecordJob.set(queue: :b).perform_later("b#{i}") }
trips = server("weighted", "-q", "a,2", "-q", "b,1", "-c", "1", "-t", "1") do |pid|
  wait_for("weighted jobs") { redis("LLEN", "oracle:ran") >= 200 }
  Process.kill("TERM", pid)
end
fetches = kept(trips).flat_map { |t| t["commands"] }.select { |c| c[0].casecmp?("brpop") }.first(200)
raise "only #{fetches.size} weighted fetches" unless fetches.size == 200
first_a = fetches.count { |c| c[1] == "queue:a" }

# --- shutdown ---------------------------------------------------------
redis("FLUSHDB")
SlowJob.perform_later("s1")
RecordJob.perform_later("after")
pushed = redis("LRANGE", "queue:default", 0, -1)
slow_raw = pushed.last
timeline = []
trips = server("shutdown", "-q", "default", "-c", "1", "-t", "1") do |pid|
  wait_for("the slow job to start") { redis("LRANGE", "oracle:ran", 0, -1).include?("s1 started") }
  Process.kill("TSTP", pid)
  sleep 0.5
  timeline << "after TSTP: queue:default holds #{redis("LLEN", "queue:default")}"
  Process.kill("TERM", pid)
end
shutdown = kept(trips)
requeue = shutdown.flat_map { |t| t["commands"] }.find { |c| c[0].casecmp?("rpush") }
raise "no requeue in #{shutdown.inspect}" unless requeue
timeline << "after TERM: queue:default holds #{redis("LLEN", "queue:default")}"

puts JSON.pretty_generate(
  "versions" => versions,
  "strict" => {
    "args" => "-q high -q low -c 1 -t 1",
    "ran_in_order" => strict_ran,
    "popped_from_high_pinned" => pushed_high,
    "round_trips" => describe_trips(strict),
    "dead_after" => dead_after
  },
  "weighted" => {
    "args" => "-q a,2 -q b,1 -c 1 -t 1",
    "fetches" => 200,
    "brpop_shapes" => fetches.map { |c| [c[0].downcase, c[1..-2].sort, c[-1]] }.uniq,
    "orders_seen" => fetches.map { |c| c[1..-2] }.uniq.sort,
    # 2/3 expected; the bounds are wide enough that a run never flakes.
    "queue_a_first_share_between_0.5_and_0.85" => (100..170).cover?(first_a)
  },
  "shutdown" => {
    "args" => "-q default -c 1 -t 1",
    "timeline" => timeline,
    "round_trips" => describe_trips(shutdown).reject { |t| t["commands"].all? { |c| c["argv"][0] == "brpop" } },
    "requeue_is_the_pushed_text" => requeue[2..] == [slow_raw]
  }
)
