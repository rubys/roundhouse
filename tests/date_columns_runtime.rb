# Independent native/emitted consumer; all databases are local and in memory.
require "date"

if ARGV.first == "native"
  require "active_record"
  ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
  ActiveRecord::Schema.define do
    create_table :calendar_entries do |t|
      t.date :due_on
      t.datetime :observed_at
      t.time :opens_at
    end
  end
  class ApplicationRecord < ActiveRecord::Base
    self.abstract_class = true
  end
  load File.join(__dir__, "date_columns_model.rb")
end

def equal(expected, actual)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
end

def date_value(expected, actual)
  equal(Date, actual.class)
  equal(expected, actual.iso8601)
end

def stored_date(id)
  CalendarEntry.connection.select_all("SELECT due_on FROM calendar_entries WHERE id = #{id}").first["due_on"]
end

entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31),
  observed_at: Time.utc(2024, 1, 31, 23, 47, 19, 123456),
  opens_at: Time.utc(2000, 1, 1, 17, 26, 43))
date_value("2024-01-31", entry.due_on)
equal("2024-01-31", stored_date(entry.id))
entry = CalendarEntry.find(entry.id)
date_value("2024-01-31", entry.due_on)
date_value("2024-01-31", entry[:due_on])
date_value("2024-01-31", entry.shifted_attribute(0))
date_value("2024-02-29", entry.shifted_index(1))
date_value("2024-02-29", entry.shifted_attribute(1))
date_value("2024-01-31", entry[:due_on]) # index shifts must not mutate storage
date_value("2024-02-29", entry.shifted(1))
date_value("2024-03-31", entry.shifted(2)) # one-shot, not repeated February clamping
date_value("2024-01-31", entry.shifted(0))
date_value("2023-12-31", entry.shifted(-1))
date_value("2044-01-31", entry.shifted(240))
date_value("2004-01-31", entry.shifted(-240))
date_value("2024-01-31", entry.due_on) # shifts must not mutate the receiver
date_value("2024-03-31", entry.reset_date)
date_value("2025-02-28", entry.parsed_date)
equal({"due_on" => "2024-01-31"}, entry.as_json(only: [:due_on]))
equal(%w[due_on id observed_at opens_at], entry.as_json.keys.sort)
equal("2024-01-31", entry.as_json["due_on"])
if ARGV.first == "native"
  Time.use_zone("Pacific/Auckland") do
    date_value("2024-01-31", entry.due_on)
    equal("2024-01-31", entry.as_json["due_on"])
  end
else
  equal("2024-01-31", entry.due_on_raw)
  equal("2024-01-31", entry.attributes["due_on"]) # attributes remains stored text
  equal(String, entry[:observed_at].class) # timestamp index behavior is unchanged
  equal(String, entry[:opens_at].class)
  ActiveSupport.use_zone("Pacific/Auckland") do
    date_value("2024-01-31", entry.due_on)
    equal("2024-01-31", entry.as_json["due_on"])
    equal('{"due_on":"2024-01-31","observed_at":"2024-02-01T12:47:19.123+13:00"}',
      ActionController::JsonRender.encode(entry.as_json(only: [:due_on, :observed_at])))
  end
  equal('{"due_on":"2024-01-31"}', ActionController::JsonRender.encode(entry.as_json(only: [:due_on])))
  equal(nil, ActiveSupport.parse_db_date(nil))
  equal(nil, ActiveSupport.parse_db_date(""))
  date_value("2024-02-29", ActiveSupport.parse_db_date("2024-02-29"))
  begin
    ActiveSupport.parse_db_date("2023-02-29")
    raise "invalid calendar date was silently accepted"
  rescue Date::Error
  end
  [17, Time.utc(2024, 1, 31)].each do |invalid|
    begin
      ActiveSupport.format_db_date(invalid)
      raise "non-date value was silently accepted"
    rescue TypeError
    end
  end
end
equal(Time, entry.observed_at.class)
equal("2024-01-31 23:47:19.123456", entry.observed_at.getutc.strftime("%Y-%m-%d %H:%M:%S.%6N"))
equal(Time, entry.opens_at.class)
equal("17:26:43", entry.opens_at.getutc.strftime("%H:%M:%S"))
equal(false, entry.observed_at.respond_to?(:>>))

entry.update!(due_on: Date.new(2023, 1, 31))
date_value("2023-01-31", entry.due_on)
date_value("2023-02-28", entry.shifted(1))
entry.reload
date_value("2023-01-31", entry.due_on)
equal("2023-01-31", stored_date(entry.id))
other = CalendarEntry.find(entry.id)
other.update!(due_on: Date.new(2024, 2, 29))
entry.reload # reloading an already-read instance must not keep a stale date
date_value("2024-02-29", entry.due_on)
date_value("2024-02-29", entry[:due_on])
date_value("2024-03-29", entry.shifted_attribute(1))
date_value("2025-02-28", entry.shifted(12))
entry.due_on = Date.new(2024, 3, 31)
entry.save!
entry.reload
date_value("2024-02-29", entry.shifted(-1))
entry.update!(due_on: nil)
equal(nil, entry.due_on)
equal(nil, stored_date(entry.id))
equal(nil, entry[:due_on])
equal(nil, entry.shifted_index(1))
equal(nil, entry.shifted_attribute(1))
equal({"due_on" => nil}, entry.as_json(only: [:due_on]))
equal('{"due_on":null}', ActionController::JsonRender.encode(entry.as_json(only: [:due_on]))) unless ARGV.first == "native"
equal(nil, entry.shifted(240))
entry.reload
equal(nil, entry.due_on)
equal(nil, entry.shifted(-1))
empty = CalendarEntry.create!
equal(nil, CalendarEntry.find(empty.id).due_on)
entry.update!(due_on: "2025-05-30") # canonical date text at the parameter seam
date_value("2025-05-30", entry.reload.due_on)
equal("2025-05-30", stored_date(entry.id))
entry.update!(observed_at: Time.utc(2025, 6, 7, 3, 41, 29))
date_value("2025-05-30", entry.reload.due_on) # unrelated PATCH must preserve Date
equal(Time, entry.observed_at.class)
equal("2025-06-07 03:41:29", entry.observed_at.getutc.strftime("%Y-%m-%d %H:%M:%S"))
datetime = DateTime.iso8601("2024-02-29T00:47:19.123456+05:30")
equal(DateTime, datetime.class)
equal("2025-02-28T00:47:19+05:30", (datetime >> 12).iso8601)
entry.update!(due_on: datetime)
date_value("2024-02-29", entry.reload.due_on)
equal("2024-02-29", stored_date(entry.id)) # date column drops clock/offset, not UTC-converted
date_value("2024-02-29", Date.civil(2024, 2, 29))
date_value("2024-02-29", Date.parse("2024-02-29", false))
date_value("2024-02-29", Date.strptime("2024-02-29", "%Y-%m-%d"))
equal(Date, Date.today.class)
entry.destroy!
equal(nil, CalendarEntry.find_by(id: entry.id))
puts "Date CRUD and month shifts OK"
