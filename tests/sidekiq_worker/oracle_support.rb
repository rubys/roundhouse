# Shared by the Sidekiq oracles (oracle.rb, retry_oracle.rb): the
# envelope's keys in written order, and the masks for values that change
# from run to run.
require "json"

def ms?(v) = v.is_a?(Integer) && v.to_s.length == 13

# A timestamp the run itself wrote, as opposed to one the oracle seeded
# (those are in 2001, and are kept so the record shows they survive).
def fresh_ms?(v) = ms?(v) && (v - (Time.now.to_f * 1000).to_i).abs < 86_400_000

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

def mask_envelope(raw)
  env = JSON.parse(raw)
  raise "jid shape: #{env["jid"]}" unless env["jid"].match?(/\A[0-9a-f]{24}\z/)
  env["jid"] = "<jid>"
  %w[created_at enqueued_at failed_at retried_at discarded_at].each do |k|
    next unless env.key?(k)
    raise "#{k} is not integer ms: #{env[k].inspect}" unless ms?(env[k])
    env[k] = "<ms>" if fresh_ms?(env[k])
  end
  env["args"] = env["args"].map do |aj|
    aj = aj.dup
    raise "job_id shape" unless aj["job_id"].match?(/\A\h{8}-\h{4}-4\h{3}-[89ab]\h{3}-\h{12}\z/)
    aj["job_id"] = "<uuid>"
    raise "enqueued_at shape" unless aj["enqueued_at"].match?(/\A\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{9}Z\z/)
    aj["enqueued_at"] = "<iso8601(9)>"
    unless aj["scheduled_at"].nil?
      raise "scheduled_at shape" unless aj["scheduled_at"].match?(/\A\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{9}Z\z/)
      aj["scheduled_at"] = "<iso8601(9)>"
    end
    aj
  end
  env
end

# A job's text with its per-run values pinned to fixed ones, keys and
# formatting as Sidekiq wrote them, so a test can feed the same text to
# the runtime and compare what it writes with what Sidekiq wrote.
# Malformed JSON is returned as it is. With `failures`, a failed job's
# `failed_at` and `retried_at` are pinned too; oracle.rb keeps its
# seeded ones, which are from 2001.
PINNED = {
  "jid" => "0123456789abcdef01234567",
  "created_at" => 1_792_632_730_123,
  "enqueued_at" => 1_792_632_730_124,
  "job_id" => "00000000-0000-4000-8000-000000000000",
  "aj_enqueued_at" => "2026-10-23T00:00:00.000000000Z",
  "aj_scheduled_at" => "2026-10-23T00:00:05.000000000Z",
  "failed_at" => 1_792_632_725_000,
  "retried_at" => 1_792_632_729_000
}.freeze

def pin(raw, failures: false)
  env = JSON.parse(raw)
  if failures
    env["failed_at"] = PINNED["failed_at"] if env.key?("failed_at")
    env["retried_at"] = PINNED["retried_at"] if env.key?("retried_at")
  end
  env["jid"] = PINNED["jid"]
  env["created_at"] = PINNED["created_at"] if env.key?("created_at")
  env["enqueued_at"] = PINNED["enqueued_at"] if env.key?("enqueued_at")
  env["args"] = env["args"].map do |aj|
    aj = aj.merge("job_id" => PINNED["job_id"], "enqueued_at" => PINNED["aj_enqueued_at"])
    aj["scheduled_at"] = PINNED["aj_scheduled_at"] unless aj["scheduled_at"].nil?
    aj
  end
  JSON.generate(env)
rescue JSON::ParserError
  raw
end
