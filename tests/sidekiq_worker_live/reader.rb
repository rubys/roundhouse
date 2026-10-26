# Reads what tests/sidekiq_worker_live/driver.rb left, through real
# Sidekiq's API, as a Sidekiq process or its Web UI would. `flush`
# empties the scratch database first; `check` prints one ok/FAIL line per
# check.
require "sidekiq"
require "sidekiq/api"

url = ENV.fetch("REDIS_URL")
Sidekiq.configure_client { |config| config.redis = { url: url } }

if ARGV[0] == "flush"
  Sidekiq.redis { |conn| conn.call("FLUSHDB") }
  puts "flushed"
  exit
end

def check(name, ok, detail = "")
  puts (ok ? "ok   " : "FAIL ") + name + (ok ? "" : " (#{detail})")
end

now = Time.now.to_f
recent = ->(t) { t.is_a?(Time) && (now - t.to_f).abs < 120 }

dead = Sidekiq::DeadSet.new.to_a.sort_by(&:display_class)
check("dead set size", dead.size == 2, dead.size.to_s)
boom, mailer = dead
if boom
  check("boom: display_class", boom.display_class == "BoomJob", boom.display_class.inspect)
  check("boom: display_args", boom.display_args == ["b1"], boom.display_args.inspect)
  check("boom: error_class", boom["error_class"] == "ArgumentError", boom["error_class"].inspect)
  check("boom: error_message", boom["error_message"] == "boom b1", boom["error_message"].inspect)
  check("boom: retry_count", boom["retry_count"] == 0, boom["retry_count"].inspect)
  check("boom: failed_at", recent.(boom.failed_at), boom.failed_at.inspect)
  check("boom: queue", boom.queue == "default", boom.queue.inspect)
  check("boom: error?", boom.error?, "")
  check("boom: dead score", (boom.score - now).abs < 120, boom.score.inspect)
end
if mailer
  check("unknown job class: display_class", mailer.display_class == "MailerJob", mailer.display_class.inspect)
  check("unknown job class: error_class", mailer["error_class"] == "NameError", mailer["error_class"].inspect)
  check("unknown job class: error_message", mailer["error_message"] == "uninitialized constant MailerJob", mailer["error_message"].inspect)
  check("unknown job class: queue", mailer.queue == "low", mailer.queue.inspect)
end

default = Sidekiq::Queue.new("default").to_a
check("the slow job is back on default", default.size == 1 && default.first.display_class == "SlowJob", default.map(&:display_class).inspect)
if (slow = default.first)
  check("requeued unchanged: no error fields", slow["error_class"].nil? && slow["retry_count"].nil?, slow.item.keys.inspect)
  check("requeued unchanged: enqueued_at", recent.(slow.enqueued_at), slow.enqueued_at.inspect)
end
check("low is empty", Sidekiq::Queue.new("low").size == 0, Sidekiq::Queue.new("low").size.to_s)
check("nothing retried", Sidekiq::RetrySet.new.size == 0, Sidekiq::RetrySet.new.size.to_s)
puts "done"
