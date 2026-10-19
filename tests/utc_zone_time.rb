require "time"
require "open3"
require "rbconfig"

zones = ["UTC", "Etc/UTC", "Etc/Universal", "Etc/Zulu", "Etc/UCT", "GMT", "Etc/GMT", "Europe/London", "Asia/Tokyo", "America/New_York"]
instants = [Time.utc(2026, 1, 2, 3, 4, 5, 123456), Time.utc(2026, 7, 2, 3, 4, 5, 123456)]
oracle = <<~RUBY
  gem "activesupport", "8.1.4"
  require "active_support/all"
  rows = #{instants.map(&:to_f).inspect}.flat_map do |epoch|
    instant = Time.at(epoch).getutc
    #{zones.inspect}.map do |name|
      Time.define_singleton_method(:now) { instant }
      Time.use_zone(name) do
        zoned = instant.in_time_zone
        local = Time.zone.local(2026, instant.month, 2, 3, 4, 5 + Rational(123456, 1000000))
        values = [zoned, zoned, zoned, local, Time.zone.now]
        [name, values.map { |time| [time.iso8601, time.iso8601(6), time.utc?] }]
      end
    end
  end
  p rows
RUBY
expected, status = Open3.capture2e(RbConfig.ruby, "-e", oracle)
abort expected unless status.success?

require ARGV.fetch(0)
require ARGV.fetch(1)
rows = instants.map(&:to_f).flat_map do |epoch|
  instant = Time.at(epoch).getutc
  zones.map do |name|
    Time.define_singleton_method(:now) { instant }
    ActiveSupport.set_default_zone(name)
    values = [
      ActiveSupport.present(instant),
      ActiveSupport.in_time_zone(instant, name),
      ActiveSupport.parse_db_time(instant.strftime("%Y-%m-%d %H:%M:%S.%6N")),
      ActiveSupport.local_time(2026, instant.month, 2, 3, 4, 5, 123456000),
      ActiveSupport::TimeZone.lookup(name).now
    ]
    [name, values.map { |time| [time.iso8601, time.iso8601(6), time.utc?] }]
  end
end
abort "zone times differ:\nexpected #{expected}\nactual #{rows.inspect}" unless rows.inspect == expected.strip

input = Time.new(2026, 1, 2, 3, 4, 5, "+09:00")
before = [input.to_i, input.utc_offset, input.utc?]
ActiveSupport.in_time_zone(input, "UTC")
abort "zone conversion mutated its input" unless [input.to_i, input.utc_offset, input.utc?] == before
abort "nil database time changed" unless ActiveSupport.present_db(nil).nil?
abort "fixed-zero control changed" unless instants.first.getlocal(0).iso8601(6).end_with?("+00:00")
puts "UTC presentation matches Rails 8.1.4: #{zones.length} zones, winter and summer, five conversion paths"
