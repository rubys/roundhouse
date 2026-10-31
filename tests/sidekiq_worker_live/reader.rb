# Reads what tests/sidekiq_worker_live/driver.rb left, through real
# Sidekiq's API, as a Sidekiq process or its Web UI would. `flush`
# empties the scratch database first; `check` prints one ok/FAIL line per
# check; `sizes` prints the default queue and the dead set, for the
# shutdown test's worker processes.
require "sidekiq"
require "sidekiq/api"

url = ENV.fetch("REDIS_URL")
Sidekiq.configure_client { |config| config.redis = { url: url } }

if ARGV[0] == "flush"
  Sidekiq.redis { |conn| conn.call("FLUSHDB") }
  puts "flushed"
  exit
end

if ARGV[0] == "sizes"
  puts "default=#{Sidekiq::Queue.new("default").size} dead=#{Sidekiq::DeadSet.new.size} " \
    "classes=#{Sidekiq::Queue.new("default").map(&:display_class).sort.join(",")}"
  exit
end

def check(name, ok, detail = "")
  puts (ok ? "ok   " : "FAIL ") + name + (ok ? "" : " (#{detail})")
end

now = Time.now.to_f
recent = ->(t) { t.is_a?(Time) && (now - t.to_f).abs < 120 }

failed = Sidekiq::RetrySet.new.to_a.sort_by(&:display_class)
check("retry set size", failed.size == 2, failed.size.to_s)
boom, mailer = failed
if boom
  check("boom: display_class", boom.display_class == "BoomJob", boom.display_class.inspect)
  check("boom: display_args", boom.display_args == ["b1"], boom.display_args.inspect)
  check("boom: error_class", boom["error_class"] == "ArgumentError", boom["error_class"].inspect)
  check("boom: error_message", boom["error_message"] == "boom b1", boom["error_message"].inspect)
  check("boom: retry_count", boom["retry_count"] == 0, boom["retry_count"].inspect)
  check("boom: failed_at", recent.(boom.failed_at), boom.failed_at.inspect)
  check("boom: queue", boom.queue == "default", boom.queue.inspect)
  check("boom: error?", boom.error?, "")
  # Sidekiq's first backoff: 15 s, plus up to 10 s of jitter, counted
  # from the failure, so a slow run may read it with little time left.
  check("boom: retry score", boom.score - now > 0 && boom.score - now < 26, (boom.score - now).inspect)
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
check("nothing dead", Sidekiq::DeadSet.new.size == 0, Sidekiq::DeadSet.new.size.to_s)
puts "done"
