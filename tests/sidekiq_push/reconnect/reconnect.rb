# The real SidekiqRedis (runtime/spinel/sidekiq/sidekiq_redis.rb) over
# the scripted Redis beside it: which failures take the one reconnect.
# tests/sidekiq_push.rs runs it under CRuby with -I. so `require
# "redis"` finds the stand-in.
require "redis"

module SidekiqPush
  def self.password
    ""
  end
end

require_relative "sidekiq_redis"

def check(name, ok, detail)
  puts (ok ? "ok   " : "FAIL ") + name + (ok ? "" : " (" + detail + ")")
end

def attempt(plan)
  SidekiqRedis::CONN.clear
  Redis::PLAN.replace(plan)
  Redis::LOG.clear
  begin
    yield
    "pushed"
  rescue StandardError => e
    e.class.name
  end
end

URL = "redis://localhost:6379/0"
push = proc { SidekiqRedis.push(URL, "default", "{}") }
schedule = proc { SidekiqRedis.schedule(URL, 1.0, "{}") }

got = attempt([:refuse], &push)
check("first connect refused once: pushed", got == "pushed", got)
check("first connect refused once: connected twice", Redis::LOG == %w[connect connect ping pipeline], Redis::LOG.join(","))

got = attempt([nil, :refuse], &push)
check("first PING fails once: pushed", got == "pushed", got)

got = attempt([:refuse, :refuse], &push)
check("connect refused twice: raises", got == "Redis::CannotConnectError", got)

got = attempt([nil, nil, :drop], &push)
check("socket drops mid-push: pushed on a new one", got == "pushed" && Redis::LOG.count("connect") == 2 && Redis::LOG.include?("close"), got + " " + Redis::LOG.join(","))

got = attempt([nil, nil, :wrong], &push)
check("command error: not retried", got == "Redis::CommandError" && Redis::LOG.count("pipeline") == 1, got)

got = attempt([:refuse], &schedule)
check("schedule: first connect refused once", got == "pushed" && Redis::LOG.last == "zadd", got)

got = attempt([:refuse, :refuse], &schedule)
check("schedule: connect refused twice raises", got == "Redis::CannotConnectError", got)

puts "done"
