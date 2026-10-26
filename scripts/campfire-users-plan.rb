# scripts/campfire-users-plan.rb DB COOKIES OUT — the plan `loadgen users`
# reads, built from the fixture database rather than from any server:
# each user's signed session cookie and rooms, every room's message ids
# (oldest first), and the search terms.
#
# Line i of COOKIES is user i in id order (`campfire-oracle cookies`
# mints them that way). sqlite3's CLI, not a gem, so this runs under any
# Ruby on the box.

require "json"
require "open3"

db, cookies, out = ARGV
abort "usage: campfire-users-plan.rb DB COOKIES OUT" unless db && cookies && out

def sql(db, query)
  stdout, stderr, status = Open3.capture3("sqlite3", "-readonly", "-json", db, query)
  abort "sqlite3: #{stderr}" unless status.success?
  stdout.strip.empty? ? [] : JSON.parse(stdout)
end

user_ids = sql(db, "SELECT id FROM users ORDER BY id").map { |r| r["id"] }
lines = File.readlines(cookies, chomp: true).reject(&:empty?)
abort "#{lines.size} cookies for #{user_ids.size} users" if lines.size < user_ids.size

rooms_of = Hash.new { |h, k| h[k] = [] }
sql(db, "SELECT user_id, room_id FROM memberships WHERE involvement != 'invisible' ORDER BY room_id").each do |r|
  rooms_of[r["user_id"]] << r["room_id"]
end

messages = Hash.new { |h, k| h[k] = [] }
sql(db, "SELECT id, room_id FROM messages ORDER BY room_id, created_at, id").each do |r|
  messages[r["room_id"]] << r["id"]
end

users = user_ids.each_with_index.map do |id, i|
  cookie = lines[i].start_with?("session_token=") ? lines[i] : "session_token=#{lines[i]}"
  { id: id, cookie: cookie, rooms: rooms_of[id] }
end

# Words the seed's message bodies contain (campfire-oracle-seed.rb BODIES).
terms = %w[coffee deploy fix lunch index test numbers standup]

File.write(out, JSON.generate(users: users, rooms: messages.transform_keys(&:to_s), terms: terms))
warn "plan: #{users.size} users, #{messages.size} rooms, #{messages.values.sum(&:size)} messages -> #{out}"
