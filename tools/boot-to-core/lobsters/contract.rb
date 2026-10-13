# frozen_string_literal: true

require "json"
lane, source = ARGV
if source == "original"
  require File.join(ENV.fetch("LOBSTERS_APP"), "config/environment")
else
  load source
  raise "Core/emitted contract imported Rails" if defined?(Rails)
end

checks = 0
observations = []
equal = ->(actual, expected) do
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
  checks += 1
  observations << actual
end

case lane
when "search", "full_capture"
  a, b = Search.new, Search.new
  equal.call([a.q, a.what, a.order, a.page, a.per_page, a.total_results, a.page_count, a.persisted?],
    ["", "stories", "newest", 1, 20, -1, 5, false])
  a.q = "λ & ruby"
  a.order = "points"
  a.page = 3
  a.what = "comments"
  equal.call([a.q, a.order, a.page, a.what], ["λ & ruby", "points", 3, "comments"])
  a.what = "unknown"
  equal.call(a.what, "stories")
  a.per_page = 7
  # Literals derived by integer division, not by reading the implementation.
  [[-1, 15], [0, 0], [1, 1], [7, 1], [8, 2], [98, 14], [99, 15], [100, 15], [101, 15], [213, 15]].each do |total, pages|
    a.total_results = total
    equal.call(a.page_count, pages)
  end
  equal.call([b.q, b.what, b.order, b.page, b.per_page, b.total_results, b.page_count],
    ["", "stories", "newest", 1, 20, -1, 5])
  b.per_page = 33
  b.total_results = 100
  equal.call(b.page_count, 4)
  equal.call(a.per_page, 7)
when "short_id"
  # klass is only an opaque scalar cell in this accessor cut: generate/valid?
  # are NOT exported or called, so no fake class/exists? protocol is introduced.
  a, b = ShortId.new("north λ"), ShortId.new("south")
  equal.call([a.klass, a.generation_attempts, b.klass, b.generation_attempts], ["north λ", 0, "south", 0])
  a.klass = "east"
  a.generation_attempts = 9
  equal.call([a.klass, a.generation_attempts], ["east", 9])
  equal.call([b.klass, b.generation_attempts], ["south", 0])
  b.generation_attempts = -3
  equal.call([a.generation_attempts, b.generation_attempts], [9, -3])
when "namespace"
  a, b = ShortId::CandidateId.new(nil), ShortId::CandidateId.new(nil)
  # Actual RNG executes; do not substitute a seeded/fake generator.
  equal.call(!!a.id.match(/\A[0-9a-z]{6}\z/), true)
  equal.call(!!b.id.match(/\A[0-9a-z]{6}\z/), true)
  a.id = "north-17"
  b.id = "south-29"
  equal.call([a.id, a.to_s, b.id, b.to_s], ["north-17", "north-17", "south-29", "south-29"])
  equal.call(a.klass, nil)
when "pagination"
  helper = Object.new.extend(ApplicationHelper)
  [[0, 1, []], [4, 2, [1, 2, 3, 4]],
   [15, 8, (1..15).to_a], [16, 1, (1..13).to_a + ["...", 16]],
   [40, 20, [1, "..."] + (14..26).to_a + ["...", 40]],
   [40, 39, [1, "..."] + (28..40).to_a],
   [16, 8, (1..14).to_a + ["...", 16]]].each { |max, cur, expected|
    equal.call(helper.page_numbers_for_pagination(max, cur), expected)
  }
when "paginator"
  a, b = StoriesPaginator.new(nil), StoriesPaginator.new(nil, 3, nil)
  equal.call([a.per_page, b.per_page], [25, 25])
  a.per_page = 7
  equal.call([a.per_page, b.per_page], [7, 25])
  b.per_page = 31
  equal.call([a.per_page, b.per_page], [7, 31])
  [[], [nil, 3, nil, 17]].each do |args|
    rejected = false
    begin
      StoriesPaginator.new(*args)
    rescue ArgumentError
      rejected = true
    end
    equal.call(rejected, true)
  end
when "inherited"
  a, b = TimeSeries.new({}), TimeSeries.new({})
  equal.call([a.x_label_format, a.width, b.width], ["%Y-%m-%d %H:%M:%S", 500, 500])
  a.width = 713
  a.x_label_format = "%Y/%m/%d"
  equal.call([a.width, a.x_label_format, b.width], [713, "%Y/%m/%d", 500])
  a.add_data(data: [Time.utc(2024, 1, 2).to_i, 17, Time.utc(2024, 1, 4).to_i, 29], title: "north")
  a.timescale_divisions = "1 day"
  equal.call(a.get_x_labels, ["2024/01/02", "2024/01/03", "2024/01/04"])
when "delegate"
  # Use the benchmark's own real SQLite backup recipe. Its schema.rb retains
  # MySQL :unsigned options rejected by this frozen Rails SQLite adapter.
  # Never patch the schema or create a substitute table/attribute protocol.
  file_db = SQLite3::Database.new(File.join(ENV.fetch("LOBSTERS_APP"), "db/production.sqlite3"), readonly: true)
  backup = SQLite3::Backup.new(ActiveRecord::Base.connection.raw_connection, "main", file_db, "main")
  backup.step(-1)
  backup.finish
  file_db.close
  a, b = User.new(username: "north17"), User.new(username: "south29")
  note = ModNote.new(user: a)
  equal.call(note.username, "north17")
  note.user = b
  equal.call(note.username, "south29")
  equal.call(a.username, "north17")
when "dynamic"
  a = Search.new
  a.q = "ruby & rails"
  a.what = "comments"
  a.order = "points"
  equal.call(a.to_url_params, "q=ruby+%26+rails&amp;what=comments&amp;order=points")
  a.q = "λ/17"
  a.what = "not-stories"
  equal.call(a.to_url_params, "q=%CE%BB%2F17&amp;what=stories&amp;order=points")
else
  raise "unknown contract #{lane}"
end
puts "LOBSTERS_CONTRACT=" + JSON.generate(checks: checks, observations: observations)
