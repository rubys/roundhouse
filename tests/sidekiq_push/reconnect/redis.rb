# A stand-in for the redis gem, for reconnect.rb: `Redis.new` and the
# commands follow the script in FakeRedis::PLAN, one step per call, so
# a connect or a command can fail on cue.
class Redis
  class BaseConnectionError < StandardError; end
  class CannotConnectError < BaseConnectionError; end
  class ConnectionError < BaseConnectionError; end
  class CommandError < StandardError; end

  PLAN = []
  LOG = []

  def self.step(what)
    LOG.push(what)
    case PLAN.shift
    when :refuse then raise CannotConnectError, "connection refused"
    when :drop then raise ConnectionError, "connection reset"
    when :wrong then raise CommandError, "WRONGTYPE"
    end
  end

  def initialize(**_options)
    Redis.step("connect")
    @closed = false
  end

  def ping
    Redis.step("ping")
    "PONG"
  end

  def pipelined
    Redis.step("pipeline")
    yield self
  end

  def sadd(*); end
  def lpush(*); end

  def zadd(*)
    Redis.step("zadd")
  end

  def close
    LOG.push("close")
  end
end
