# The worker role (runtime/spinel/sidekiq_worker.rb) against a scripted
# SidekiqRedis, so no Redis is involved: the script is the jobs BRPOP
# would return, and every write the worker makes is logged as text.
#
# tests/sidekiq_worker.rs copies this file beside the runtime sources it
# requires, writes popped.txt (the oracle's strict-scenario jobs), and
# runs it under CRuby and, compiled by spinel, as a binary. It prints
# one `ok`/`FAIL` line per check and the writes, with per-run values
# masked, then a `rule` line for each of the retry oracle's failures
# (failures.txt), the `sched` lines for the scheduler moving the
# oracle's due jobs (moves.txt), then `done`.
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

# The broadcast log a model's after-commit broadcasts append to
# (runtime/spinel/broadcasts.rb). Every job below broadcasts once, and
# counts it when the log it starts with isn't empty: the worker gives
# each job a fresh log, or the log grows by one entry a job.
module Broadcasts
  LOG = [""]
  STALE = [0]
  SENT = [0]

  def self.reset_log!
    LOG.clear
    nil
  end

  def self.job_broadcasts(job_class)
    STALE[0] += 1 if LOG.length > 0
    SENT[0] += 1
    LOG.push(job_class)
    nil
  end
end

# The scripted Redis. SCRIPT holds "key<TAB>job" entries; a fetch takes
# the first entry for the first key that has one, as BRPOP does.
module SidekiqRedis
  SCRIPT = [""]
  LOG = [""]
  ORDERS = [""]
  QUIET_ON_FETCH = [false]
  L = Mutex.new
  OPENED = [0]

  def self.push(url, queue, json)
    raise "the worker driver pushes nothing"
  end

  def self.schedule(url, at, json)
    raise "the worker driver schedules nothing"
  end

  def self.worker_open(url)
    L.synchronize { OPENED[0] = OPENED[0] + 1 }
    OPENED[0]
  end

  def self.fetch(conn, keys, timeout)
    found = []
    L.synchronize do
      order = keys.join(",")
      ORDERS.push(order) unless ORDERS.include?(order)
      keys.each do |key|
        if found.length == 0
          i = 0
          while i < SCRIPT.length
            tab = SCRIPT[i].index("\t")
            if SCRIPT[i][0, tab] == key
              found.push(key)
              found.push(SCRIPT[i][tab + 1, SCRIPT[i].length - tab - 1])
              SCRIPT.delete_at(i)
              i = SCRIPT.length
            end
            i += 1
          end
        end
      end
    end
    if found.length == 0
      sleep 0.01
    elsif QUIET_ON_FETCH[0]
      SidekiqWorker::QUIET[0] = true
    end
    found
  end

  def self.requeue(conn, keys, jobs)
    L.synchronize { LOG.push("requeue " + keys.join(",") + " " + jobs.join(" | ")) }
    nil
  end

  # The delay is checked against Sidekiq's backoff for the record's
  # retry_count, then dropped: it has jitter in it.
  def self.write_retry(conn, at, json)
    count = JSON.parse(json)["retry_count"].to_i
    delay = at - Time.now.to_f
    ok = delay > count**4 + 14 && delay < count**4 + 16 + 10 * (count + 1)
    L.synchronize { LOG.push("retry " + (ok ? "delay ok " : "delay " + delay.to_s + " ") + mask(json)) }
    nil
  end

  def self.write_dead(conn, now, json, oldest, keep)
    age = (now - oldest.to_f).round.to_s
    L.synchronize { LOG.push("dead " + mask(json) + " trim age " + age + " keep " + keep) }
    nil
  end

  def self.close_quietly(conn)
    nil
  end

  # The scheduler's side. SETS holds "set<TAB>job" entries; EVALSHA pops
  # the first entry for its set, as the Lua pop does. SCRIPT_LOADED off
  # makes the next EVALSHA fail with NOSCRIPT, as after SCRIPT FLUSH.
  SETS = [""]
  SCRIPT_LOADED = [false]

  def self.script_load(conn, source)
    L.synchronize do
      LOG.push("script load " + (source == SidekiqScheduler::LUA_ZPOPBYSCORE ? "zpopbyscore" : "other"))
      SCRIPT_LOADED[0] = true
    end
    "000a4c2f82df69db1780053377a2c9b0741570e0"
  end

  def self.evalsha(conn, sha, key, arg)
    found = []
    flushed = false
    L.synchronize do
      flushed = !SCRIPT_LOADED[0]
      if flushed
        LOG.push("noscript")
      else
        i = 0
        while i < SETS.length
          tab = SETS[i].index("\t")
          if SETS[i][0, tab] == key
            found.push(SETS[i][tab + 1, SETS[i].length - tab - 1])
            SETS.delete_at(i)
            i = SETS.length
          end
          i += 1
        end
      end
    end
    raise "NOSCRIPT No matching script. Please use EVAL." if flushed
    found.length == 0 ? nil : found[0]
  end

  def self.scard(conn, key)
    0
  end

  def self.push_on(conn, queue, json)
    L.synchronize { LOG.push("push " + queue + " " + mask(json)) }
    nil
  end

  def self.hard_exit(code)
    L.synchronize { LOG.push("exit " + code.to_s) }
    nil
  end

  # A record with the times it was written masked.
  def self.mask(json)
    env = nil
    begin
      env = JSON.parse(json)
    rescue StandardError
      return json
    end
    return json unless env.is_a?(Hash)
    now = Process.clock_gettime(Process::CLOCK_REALTIME, :millisecond)
    ["failed_at", "retried_at", "enqueued_at"].each do |k|
      v = env[k]
      env[k] = "<now>" if !v.nil? && (now - v.to_i).abs < 60_000
    end
    JSON.generate(env)
  end

  def self.take_log
    out = L.synchronize { LOG.dup }
    L.synchronize { LOG.clear }
    out
  end
end

require_relative "active_job"
require_relative "global_id_locator"
require_relative "active_job_serialization"
require_relative "sidekiq_push"
require_relative "job_registry"
require_relative "sidekiq_envelope"
require_relative "sidekiq_worker"

module JobRegistry
  def self.perform(job_class, args)
    Broadcasts.job_broadcasts(job_class)
    if job_class == "BoomJob"
      raise ArgumentError, "boom \"" + ActiveJob::Arguments.str_at(args, 0) + "\"\nsecond line"
    elsif job_class == "SlowJob"
      sleep 3
      true
    elsif job_class == "RecordJob"
      true
    else
      false
    end
  end

  def self.discards_deserialization_error(job_class)
    false
  end
end

def check(name, ok, detail)
  puts (ok ? "ok   " : "FAIL ") + name + (ok ? "" : " (" + detail + ")")
end

SidekiqRedis::SCRIPT.clear
SidekiqRedis::LOG.clear
SidekiqRedis::ORDERS.clear
SidekiqWorker::LOGGING[0] = false

# ---- the command line ---------------------------------------------------
check("defaults", SidekiqWorker.parse(["sidekiq"]) == "" && SidekiqWorker::CONCURRENCY[0] == 5 &&
  SidekiqWorker::TIMEOUT[0] == 25 && SidekiqWorker::QUEUES == ["queue:default"] && SidekiqWorker::STRICT[0], "")
check("strict queues", SidekiqWorker.parse(["sidekiq", "-q", "high", "-q", "low", "-q", "high", "-c", "3", "-t", "1"]) == "" &&
  SidekiqWorker::QUEUES == ["queue:high", "queue:low"] && SidekiqWorker::STRICT[0] &&
  SidekiqWorker::CONCURRENCY[0] == 3 && SidekiqWorker::TIMEOUT[0] == 1, SidekiqWorker::QUEUES.join(","))
check("weighted queues", SidekiqWorker.parse(["sidekiq", "--queue", "a,2", "--queue", "b"]) == "" &&
  SidekiqWorker::QUEUES == ["queue:a", "queue:a", "queue:b"] && !SidekiqWorker::STRICT[0], SidekiqWorker::QUEUES.join(","))
check("help", SidekiqWorker.parse(["sidekiq", "-h"]) == "help", "")
check("unknown option", SidekiqWorker.parse(["sidekiq", "--verbose"]) == "unknown option --verbose", "")
check("bad concurrency", SidekiqWorker.parse(["sidekiq", "-c", "zero"]) == "-c needs a positive number, not zero", "")

first_a = 0
k = 0
while k < 3000
  keys = SidekiqWorker.fetch_keys
  first_a += 1 if keys[0] == "queue:a"
  check("weighted fetch has each queue once", false, keys.join(",")) unless keys.length == 2
  k += 1
end
check("weighted fetch puts a first about 2/3 of the time", first_a > 1800 && first_a < 2200, first_a.to_s)

# ---- running jobs, in order, on one thread --------------------------------
popped = File.read("popped.txt").split("\n")
SidekiqWorker.parse(["sidekiq", "-q", "high", "-q", "low", "-c", "1", "-t", "1"])
popped.each { |raw| SidekiqRedis::SCRIPT.push("queue:high\t" + raw) }
SidekiqRedis::SCRIPT.push("queue:low\t" + JSON.generate({ "retry" => true, "queue" => "low", "class" => "HardWorker", "args" => [1], "jid" => "aaaaaaaaaaaaaaaaaaaaaaaa" }))
SidekiqRedis::SCRIPT.push("queue:low\t" + JSON.generate({ "retry" => false, "queue" => "low", "class" => "HardWorker", "args" => [2], "jid" => "bbbbbbbbbbbbbbbbbbbbbbbb" }))
# Malformed envelopes fail their job, not the processor.
SidekiqRedis::SCRIPT.push("queue:low\tnull")
SidekiqRedis::SCRIPT.push("queue:low\t" + JSON.generate({ "retry" => true, "queue" => "low", "class" => "Sidekiq::ActiveJob::Wrapper", "wrapped" => "NoArgsJob", "jid" => "cccccccccccccccccccccccc" }))
SidekiqWorker.start(1)
waited = 0
while (SidekiqRedis::SCRIPT.length > 0 || SidekiqWorker::INFLIGHT_JOB[0].length > 0) && waited < 500
  sleep 0.01
  waited += 1
end
check("every job fetched", SidekiqRedis::SCRIPT.length == 0, SidekiqRedis::SCRIPT.length.to_s)
check("strict order sent as given", SidekiqRedis::ORDERS == ["queue:high,queue:low"], SidekiqRedis::ORDERS.join(" / "))
SidekiqWorker.install_traps
Process.kill("TERM", Process.pid)
SidekiqWorker.supervise
SidekiqWorker::THREADS.each { |t| t.join }
check("all processors stopped", SidekiqWorker.running == 0, SidekiqWorker.running.to_s)
check("each job starts with an empty broadcast log", Broadcasts::STALE[0] == 0 && Broadcasts::SENT[0] > 1,
  Broadcasts::STALE[0].to_s + " of " + Broadcasts::SENT[0].to_s + " stale")
SidekiqRedis.take_log.each { |line| puts line }

# ---- shutdown with a job still running --------------------------------------
slow = JSON.generate({ "retry" => true, "queue" => "default", "wrapped" => "SlowJob",
  "args" => [{ "job_class" => "SlowJob", "arguments" => [] }],
  "class" => "Sidekiq::ActiveJob::Wrapper", "jid" => "cccccccccccccccccccccccc" })
SidekiqWorker.parse(["sidekiq", "-c", "2", "-t", "1"])
SidekiqRedis::SCRIPT.push("queue:default\t" + slow)
SidekiqWorker.start(2)
waited = 0
while SidekiqWorker::INFLIGHT_JOB.join("").length == 0 && waited < 500
  sleep 0.01
  waited += 1
end
t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
Process.kill("TERM", Process.pid)
SidekiqWorker.supervise
took = Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0
check("shutdown waited about the timeout", took > 0.9 && took < 2.5, took.to_s)
log = SidekiqRedis.take_log
check("the running job is pushed back unchanged", log == ["requeue queue:default " + slow], log.join(" / "))
SidekiqWorker::THREADS.each { |t| t.join }

# ---- TSTP: a job popped after QUIET goes straight back ------------------------
SidekiqWorker.parse(["sidekiq", "-c", "1", "-t", "1"])
SidekiqRedis::QUIET_ON_FETCH[0] = true
SidekiqRedis::SCRIPT.push("queue:default\t" + slow)
SidekiqWorker.start(1)
SidekiqWorker::THREADS.each { |t| t.join }
SidekiqRedis::QUIET_ON_FETCH[0] = false
log = SidekiqRedis.take_log
check("a job popped while quiet is pushed back", log == ["requeue queue:default " + slow], log.join(" / "))

# ---- Sidekiq's retry rules on the jobs Sidekiq failed ------------------------
# failures.txt: the oracle's failed jobs, one per line, as
# "queue<TAB>error class<TAB>message<TAB>popped text". Each prints
# `rule` and what the worker wrote for it, or `rule nothing`.
File.read("failures.txt").split("\n").each do |line|
  parts = line.split("\t", 4)
  raw = parts[3]
  SidekiqWorker.fail_job(JSON.parse(raw), raw, parts[0], "class=Job", parts[1], parts[2], SidekiqWorker.clock)
  log = SidekiqRedis.take_log
  puts "rule " + (log.length == 0 ? "nothing" : log.join(" / "))
end

# ---- the scheduler on the jobs Sidekiq's poller moved ------------------------
# moves.txt: the oracle's moves, one per line, as "set<TAB>popped
# text". One cycle drains retry, then schedule; each push prints as
# `sched push <queue> <json>`.
SidekiqRedis::SETS.clear
File.read("moves.txt").split("\n").each { |line| SidekiqRedis::SETS.push(line) }
SidekiqRedis::SCRIPT_LOADED[0] = false
SidekiqWorker::QUIET[0] = false
SidekiqScheduler.enqueue(1)
SidekiqRedis.take_log.each { |line| puts "sched " + line }
check("every due job moved", SidekiqRedis::SETS.length == 0, SidekiqRedis::SETS.length.to_s)

# After SCRIPT FLUSH the pop fails with NOSCRIPT, loads the script
# again, and goes on.
SidekiqRedis::SCRIPT_LOADED[0] = false
SidekiqRedis::SETS.push("retry\t{\"retry\":true,\"queue\":\"low\",\"class\":\"HardWorker\",\"args\":[],\"jid\":\"dddddddddddddddddddddddd\",\"retry_count\":0}")
SidekiqScheduler.enqueue(1)
log = SidekiqRedis.take_log
check("NOSCRIPT reloads the script", log.length == 3 && log[0] == "noscript" && log[1] == "script load zpopbyscore" &&
  log[2] == "push low {\"retry\":true,\"queue\":\"low\",\"class\":\"HardWorker\",\"args\":[],\"jid\":\"dddddddddddddddddddddddd\",\"retry_count\":0,\"enqueued_at\":\"<now>\"}", log.join(" / "))

# A job that isn't a JSON object can't be pushed: dead as it is.
SidekiqRedis::SETS.push("schedule\tnot json")
SidekiqScheduler.enqueue(1)
log = SidekiqRedis.take_log
check("an unreadable job goes to dead", log == ["dead not json trim age 15552000 keep -10000"], log.join(" / "))

# QUIET stops a cycle between pops.
SidekiqRedis::SETS.push("retry\t{\"queue\":\"a\"}")
SidekiqWorker::QUIET[0] = true
SidekiqScheduler.enqueue(1)
check("QUIET stops the cycle", SidekiqRedis::SETS.length == 1 && SidekiqRedis.take_log.length == 0, "")
SidekiqRedis::SETS.clear

# The waits: poll_interval_average 1 s gives 0.5 to 1.5 s and no 10 s
# start; none gives 5 s per process (2.5 to 7.5 s) and 10 to 15 s.
SidekiqScheduler::INTERVALS[0] = 1.0
waits_ok = true
k = 0
while k < 200
  w = SidekiqScheduler.interval([])
  waits_ok = false if w < 0.5 || w >= 1.5
  w = SidekiqScheduler.initial_wait
  waits_ok = false if w < 0.0 || w >= 5.0
  k += 1
end
check("poll_interval_average waits", waits_ok, "")
SidekiqScheduler::INTERVALS[0] = -1.0
SidekiqScheduler::INTERVALS[1] = -1.0
waits_ok = true
k = 0
while k < 200
  w = SidekiqScheduler.interval([])
  waits_ok = false if w < 2.5 || w >= 7.5
  w = SidekiqScheduler.initial_wait
  waits_ok = false if w < 10.0 || w >= 15.0
  k += 1
end
check("default waits", waits_ok, "")

# Started while QUIET, the thread leaves at once.
SidekiqScheduler.start(1.0, -1.0)
SidekiqScheduler::THREAD.each { |t| t.join }
check("the scheduler stops on QUIET", !SidekiqScheduler.running?, "")
puts "done"
