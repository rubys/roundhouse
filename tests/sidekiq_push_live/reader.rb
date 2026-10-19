# Reads what tests/sidekiq_push_live/driver.rb pushed, through real
# Sidekiq's API, as a Sidekiq process or its Web UI would. `flush` empties
# the scratch database first; `check` prints one ok/FAIL line per check.
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

names = Sidekiq::Queue.all.map(&:name).sort
check("queues", names == %w[default low], names.inspect)

default = Sidekiq::Queue.new("default").to_a
low = Sidekiq::Queue.new("low").to_a
check("default queue size", default.size == 2, default.size.to_s)
check("low queue size", low.size == 1, low.size.to_s)

expected = {
  "gid://depot/Car/1" => "InspectCarJob",
  "gid://depot/Car/2" => "WashCarJob",
  "gid://depot/Car/4" => "AfterCommitJob",
}
(default + low).each do |job|
  gid = job.display_args.first
  check("#{gid}: wrapper class", job.klass == "Sidekiq::ActiveJob::Wrapper", job.klass)
  check("#{gid}: display_class", job.display_class == expected[gid], job.display_class.inspect)
  check("#{gid}: display_args", job.display_args == [gid], job.display_args.inspect)
  check("#{gid}: jid", job.jid.match?(/\A\h{24}\z/), job.jid.inspect)
  check("#{gid}: enqueued_at", recent.(job.enqueued_at), job.enqueued_at.inspect)
  check("#{gid}: created_at", recent.(job.created_at), job.created_at.inspect)
end
seen = (default + low).map { |job| job.display_args.first }.sort
check("immediate and after-commit jobs", seen == expected.keys.sort, seen.inspect)

scheduled = Sidekiq::ScheduledSet.new.to_a
check("scheduled set size", scheduled.size == 1, scheduled.size.to_s)
if (job = scheduled.first)
  check("scheduled: display_class", job.display_class == "InspectCarJob", job.display_class.inspect)
  check("scheduled: queue", job.queue == "default", job.queue.inspect)
  check("scheduled: at", (job.at.to_f - (now + 300)).abs < 120, job.at.inspect)
  check("scheduled: display_args", job.display_args == ["gid://depot/Car/3"], job.display_args.inspect)
end
check("rolled-back job never pushed", (default + low + scheduled).none? { |j| j.display_args == ["gid://depot/Car/5"] })
puts "done"
