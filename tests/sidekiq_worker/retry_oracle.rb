#!/usr/bin/env ruby
# The oracle for the worker's retry rules and the poller: what a real
# Sidekiq 8.1 server sends to Redis when a job fails, and when it moves
# due jobs from the `retry` and `schedule` sets back onto their queues.
#
#   ruby tests/sidekiq_worker/retry_oracle.rb > tests/sidekiq_worker/sidekiq_retry.json
#
# Needs activejob 8.1.3, sidekiq 8.1.7 (with redis-client and
# connection_pool) installed, and redis-server and redis-cli on PATH.
# It starts its own redis-server on a private port and runs one real
# `sidekiq` server against it with `poll_interval_average: 1`, with
# tests/sidekiq_worker/retry_oracle_jobs.rb loaded through `-r`. That
# file records every round trip the server makes, and each reply.
#
# The scenarios run one after another on that server:
#   flaky       - `retry: true`, fails twice, then succeeds
#   two_retries - `retry: 2`, always fails: two retries, then dead
#   no_retry    - `retry: false`: dropped, nothing written
#   zero_retry  - `retry: 0`: dead at once
#   retry_queue - `retry: 3, retry_queue: "low"`, fails once
#   retry_for   - `retry_for: 1`, always fails: dead once a second has passed
#   not_dead    - `retry: 1, dead: false`: dropped when exhausted
#   scheduled   - `set(wait: 1)`: moved from `schedule` when due
#   noscript    - `SCRIPT FLUSH` under the poller: it loads the script again
#
# Sidekiq's backoff is at least 15 seconds, so the oracle moves each
# retry entry's score to now (`ZADD XX`) once it is written; the score
# Sidekiq chose is checked against its formula first. Only the commands
# the worker itself must send are kept: fetches that popped a job, the
# poller's pops that found one, the pushes, and the retry and dead
# writes. Values that change per run are checked for shape and masked.
# `failures` pairs each job that failed, as it was popped (per-run
# values pinned), with what Sidekiq wrote for it, and `moves` each job
# the poller popped with the push that put it back on its queue.
require "json"
require "rbconfig"
require "tmpdir"
require_relative "oracle_support"

PORT = 16541
URL = "redis://127.0.0.1:#{PORT}/0"
DIR = File.expand_path(__dir__)
WORK = Dir.mktmpdir("sidekiq-retry-oracle")
LOG = File.join(WORK, "server.log")

ENV["ORACLE_LOG"] = File.join(WORK, "client.log")
ENV["REDIS_URL"] = URL
require_relative "retry_oracle_jobs"
Sidekiq.configure_client do |config|
  config.redis = { url: URL }
  config.logger.level = Logger::WARN
end

system("redis-server", "--port", PORT.to_s, "--save", "", "--appendonly", "no",
  "--daemonize", "yes", "--pidfile", "/tmp/sidekiq-retry-oracle.pid",
  "--logfile", "/tmp/sidekiq-retry-oracle.log", exception: true)
50.times do
  break if system("redis-cli", "-p", PORT.to_s, "ping", out: File::NULL, err: File::NULL)
  sleep 0.1
end
at_exit { system("redis-cli", "-p", PORT.to_s, "shutdown", "nosave", out: File::NULL, err: File::NULL) }

def redis(*argv) = Sidekiq.redis { |c| c.call(*argv) }

def wait_for(what, seconds = 60)
  deadline = Time.now + seconds
  until yield
    raise "timed out waiting for #{what}" if Time.now > deadline
    sleep 0.05
  end
end

def attempts = redis("LRANGE", "oracle:attempts", 0, -1)
def ran = redis("LRANGE", "oracle:ran", 0, -1)
def dead_count = redis("ZCARD", "dead")

# How long after its failure a retry entry is left alone, for retry_for.
$rescore_after = 0.0

# ZRANGE WITHSCORES answers [member, score] pairs (RESP3).
rescorer = Thread.new do
  loop do
    redis("ZRANGE", "retry", 0, -1, "WITHSCORES").each do |raw, score|
      now = Time.now.to_f
      next unless score > now
      env = JSON.parse(raw)
      failed = (env["retried_at"] || env["failed_at"]) / 1000.0
      next if now - failed < $rescore_after
      redis("ZADD", "retry", "XX", (now - 1).to_s, raw)
    end
    sleep 0.05
  end
end
rescorer.abort_on_exception = true

redis("FLUSHDB")
config = File.join(WORK, "sidekiq.yml")
File.write(config, "poll_interval_average: 1\n")
bin = Gem.bin_path("sidekiq", "sidekiq", "8.1.7")
out = File.join(WORK, "server.out")
PID = Process.spawn({ "ORACLE_LOG" => LOG, "REDIS_URL" => URL },
  RbConfig.ruby, bin, "-r", File.join(DIR, "retry_oracle_jobs.rb"), "-C", config,
  "-q", "default", "-q", "low", "-c", "1", "-t", "1", out: out, err: out)
at_exit { Process.kill("KILL", PID) rescue nil }

def trips_since(line)
  File.readlines(LOG).drop(line).map { |l| JSON.parse(l) }
end

def log_lines = File.exist?(LOG) ? File.readlines(LOG).size : 0

wait_for("the server to boot") { log_lines > 0 }
wait_for("the poller's first pop") do
  trips_since(0).any? { |t| t["commands"][0][0].casecmp?("evalsha") }
end

def float_s?(s) = s.is_a?(String) && s == s.to_f.to_s

# One kept command, described with its per-run values masked.
def describe(trip, argv, notes)
  verb = argv[0].downcase
  case verb
  when "brpop"
    { "argv" => [verb] + argv[1..], "popped_from" => trip["reply"][0] }
  when "evalsha"
    raise "evalsha shape #{argv.inspect}" unless argv[1].match?(/\A\h{40}\z/) && argv[2] == "1" && float_s?(argv[4])
    raw = trip["reply"]
    { "argv" => [verb, "<sha>", argv[2], argv[3], "<now as Float#to_s>"],
      "reply_keys" => top_level_keys(raw), "reply" => mask_envelope(raw) }
  when "zadd"
    raw = argv[3]
    entry = { "argv" => [verb, argv[1], "<score>"], "envelope_keys" => top_level_keys(raw), "envelope" => mask_envelope(raw) }
    env = JSON.parse(raw)
    if argv[1] == "retry"
      score = argv[2]
      raise "retry score #{score} is not Float#to_s" unless float_s?(score)
      count = env["retry_count"]
      at = (env["retried_at"] || env["failed_at"]) / 1000.0
      jitter = score.to_f - at - (count**4 + 15)
      raise "retry delay off: count #{count}, jitter #{jitter}" unless jitter > -1 && jitter < 10 * (count + 1) + 1
      entry["argv"][2] = "<now + count**4 + 15 + rand(10 * (count + 1)), as Float#to_s>"
    else
      raise "#{argv[1]} score #{argv[2]} is not Float#to_s" unless float_s?(argv[2])
      notes[:now] = argv[2].to_f
      entry["argv"][2] = "<now as Float#to_s>"
    end
    entry
  when "zremrangebyscore"
    { "argv" => [verb, argv[1], argv[2], "<now - #{(notes[:now] - argv[3].to_f).round}>"] }
  when "lpush"
    { "argv" => [verb, argv[1]], "envelope_keys" => top_level_keys(argv[2]), "envelope" => mask_envelope(argv[2]) }
  else
    { "argv" => [verb] + argv[1..] }
  end
end

# A command the worker must send, as opposed to heartbeats, stats, idle
# polls, the jobs' own bookkeeping, and fetches that found nothing.
def kept?(trip, argv)
  verb = argv[0].downcase
  return false if argv[1].to_s.start_with?("oracle:")
  case verb
  when "brpop" then !trip["reply"].nil?
  when "evalsha" then trip.key?("reply") && !trip["reply"].nil?
  when "zadd" then %w[retry dead schedule].include?(argv[1])
  when "sadd" then argv[1] == "queues"
  when "lpush", "zremrangebyscore", "zremrangebyrank", "multi", "exec" then true
  else false
  end
end

def describe_trips(trips)
  notes = {}
  trips.filter_map do |trip|
    commands = trip["commands"].select { |argv| kept?(trip, argv) }
    next if commands.empty? || commands.map { |c| c[0].downcase } == %w[multi exec]
    { "kind" => trip["kind"], "commands" => commands.map { |argv| describe(trip, argv, notes) } }
  end
end

# The scenarios whose last run raises and writes nothing.
SILENT_LAST_FAILURE = %w[no_retry not_dead].freeze

# Each job the server popped and failed, pinned, with the error it
# raised and what it wrote after: the retry or dead record, or nothing.
# The worker's tests feed these to the runtime's retry rules.
def failures(name, trips)
  pops = []
  trips.each do |trip|
    argv = trip["commands"][0]
    if argv[0].casecmp?("brpop") && trip["reply"]
      pops << { "queue" => trip["reply"][0].delete_prefix("queue:"), "popped" => pin(trip["reply"][1], failures: true) }
      next
    end
    next if pops.empty?
    trip["commands"].each do |c|
      next unless c[0].casecmp?("zadd") && %w[retry dead].include?(c[1])
      pops.last["wrote"] = c[1]
      pops.last["record"] = c[3]
    end
  end
  failed = pops.select { |p| p["wrote"] }
  failed << pops.last.merge("wrote" => "nothing") if SILENT_LAST_FAILURE.include?(name)
  failed.map do |p|
    raw = p["record"]
    entry = { "queue" => p["queue"], "popped" => p["popped"],
      "error_class" => raw ? JSON.parse(raw)["error_class"] : "Sticky",
      "error_message" => raw ? JSON.parse(raw)["error_message"] : "dropped",
      "wrote" => p["wrote"] }
    raw ? entry.merge("record_keys" => top_level_keys(raw), "record" => mask_envelope(raw)) : entry
  end
end

# Each job the poller popped, pinned, and the push that followed: the
# queue and the job's text with enqueued_at set.
def moves(trips)
  out = []
  trips.each do |trip|
    argv = trip["commands"][0]
    if argv[0].casecmp?("evalsha") && trip["reply"]
      out << { "set" => argv[3], "popped" => pin(trip["reply"], failures: true) }
      next
    end
    next if out.empty? || out.last.key?("pushed")
    lpush = trip["commands"].find { |c| c[0].casecmp?("lpush") && !c[1].start_with?("oracle:") }
    next unless lpush
    out.last.merge!("queue" => lpush[1].delete_prefix("queue:"),
      "pushed_keys" => top_level_keys(lpush[2]), "pushed" => mask_envelope(lpush[2]))
  end
  out
end

SCENARIOS = {}
FAILURES = {}
MOVES = {}

def scenario(name, settle: 0.5)
  start = log_lines
  yield
  sleep settle
  trips = trips_since(start)
  SCENARIOS[name] = describe_trips(trips)
  FAILURES[name] = failures(name, trips)
  MOVES[name] = moves(trips)
end

scenario("flaky") do
  FlakyJob.perform_later("flaky", 2)
  wait_for("flaky to succeed") { ran.include?("flaky") }
end

scenario("two_retries") do
  TwoRetriesJob.perform_later("two")
  wait_for("two to die") { dead_count == 1 }
end

scenario("no_retry", settle: 1.5) do
  NoRetryJob.perform_later("never")
  wait_for("never to run") { attempts.include?("never 1") }
end
raise "no_retry wrote to dead" unless dead_count == 1
raise "no_retry wrote to retry" unless redis("ZCARD", "retry") == 0

scenario("zero_retry") do
  ZeroRetryJob.perform_later("zero")
  wait_for("zero to die") { dead_count == 2 }
end

scenario("retry_queue") do
  LowRetryJob.perform_later("low", 1)
  wait_for("low to succeed") { ran.include?("low") }
end

$rescore_after = 1.5
scenario("retry_for") do
  RetryForJob.perform_later("for")
  wait_for("for to die") { dead_count == 3 }
end
$rescore_after = 0.0

scenario("not_dead", settle: 1.5) do
  NotDeadJob.perform_later("notdead")
  wait_for("notdead to run twice") { attempts.include?("notdead 2") }
end
raise "not_dead wrote to dead" unless dead_count == 3
raise "not_dead left a retry" unless redis("ZCARD", "retry") == 0

schedule_entry = nil
scenario("scheduled") do
  RecordJob.set(wait: 1).perform_later("sched")
  schedule_entry = redis("ZRANGE", "schedule", 0, -1).first
  wait_for("sched to run") { ran.include?("sched") }
end

# Every cycle the poller made: SCARD processes is only read when
# poll_interval_average is unset, so it doesn't appear here.
start = log_lines
redis("SCRIPT", "FLUSH")
wait_for("the poller to load the script again") do
  trips_since(start).any? { |t| t["commands"][0].map(&:downcase).first(2) == %w[script load] }
end
sleep 1
noscript = trips_since(start).select { |t| %w[evalsha script].include?(t["commands"][0][0].downcase) }
  .first(4).map do |t|
    argv = t["commands"][0]
    shape = if argv[0].casecmp?("script") then argv.first(2).map(&:downcase) + ["<lua>"]
    else ["evalsha", argv[1].match?(/\A\h{40}\z/) ? "<sha>" : argv[1], argv[2], argv[3], "<now>"]
    end
    { "argv" => shape, "error" => t["error"], "reply" => t["error"] ? nil : (argv[0].casecmp?("script") ? "<sha>" : t["reply"]) }.compact
  end

all = trips_since(0)
load = all.find { |t| t["commands"][0].map(&:downcase).first(2) == %w[script load] }
# A cycle starts with the first retry pop after a schedule pop. Its
# gaps include the sleep, the cycle's own pops, and pushes.
pops = all.select { |t| t["commands"][0][0].casecmp?("evalsha") }
cycles = pops.each_cons(2).select { |a, b| a["commands"][0][3] == "schedule" && b["commands"][0][3] == "retry" }.map { |_, b| b["t"] }
gaps = cycles.each_cons(2).map { |a, b| b - a }
warn "cycle gaps: min #{gaps.min.round(3)} max #{gaps.max.round(3)} over #{gaps.size}"

Process.kill("TERM", PID)
Process.wait(PID)

versions = {
  "sidekiq" => Sidekiq::VERSION,
  "activejob" => ActiveJob.version.to_s,
  "redis_client" => RedisClient::VERSION,
  "redis_server" => redis("INFO", "server")[/redis_version:(\S+)/, 1]
}

puts JSON.pretty_generate(
  "versions" => versions,
  "args" => "-C <poll_interval_average: 1> -q default -q low -c 1 -t 1",
  "poller" => {
    "script" => load["commands"][0][2],
    "sha" => load["reply"],
    "sets_in_order" => all.select { |t| t["commands"][0][0].casecmp?("evalsha") }.map { |t| t["commands"][0][3] }.first(2),
    # poll_interval_average 1 with under 10 processes sleeps 0.5 to
    # 1.5 s between cycles; a cycle's own work adds a little.
    "cycles_seen_at_least_10" => gaps.size >= 10,
    "gaps_between_cycles_within_0.45_and_2.0" => gaps.all? { |g| g > 0.45 && g < 2.0 },
    "noscript" => noscript
  },
  "scenarios" => SCENARIOS,
  "failures" => FAILURES,
  "moves" => MOVES,
  "schedule_entry" => {
    "envelope_keys" => top_level_keys(schedule_entry),
    "envelope" => mask_envelope(schedule_entry)
  },
  "attempts" => attempts,
  "ran" => ran,
  "dead_after" => redis("ZRANGE", "dead", 0, -1).map { |raw| top_level_keys(raw) }
)
