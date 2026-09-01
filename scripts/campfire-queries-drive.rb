#!/usr/bin/env ruby
# scripts/campfire-queries-drive.rb -- sign in to one campfire lane and GET a
# list of routes, then read that lane's query log back per request.
#
#   ruby campfire-queries-drive.rb walk BASE_URL ROUTE...
#       Signs in as the seeded user (after one unauthenticated GET of
#       /session/new, which is itself a measured route) and GETs each route
#       in order. Prints "path -> status bytes" per route.
#
#   ruby campfire-queries-drive.rb rails LOG ROUTE...
#       Reads a Rails log written at RAILS_LOG_LEVEL=debug and answers one
#       line per route, in walk order: the "Completed" line's query count
#       and cached count. Rails' query cache replays identical SELECTs
#       within a request, so real round trips = queries - cached.
#
#   ruby campfire-queries-drive.rb binary LOG ROUTE...
#       Reads the binary's stderr written under RH_SQL_TRACE=1: a "-- VERB
#       /path" marker per request, then one "  SQL ..." line per real
#       sqlite3 round trip and one "  CACHE ..." line per replay from the
#       request's query cache (runtime/spinel/db.rb). Answers one line per
#       route with both counts, and (to stderr) the SQL shapes behind the
#       real ones, most frequent first, so the N+1 names itself.
#
# Both readers walk the log in request order and match each measured route
# to the next request for the same path, so a route GET twice (a cold and a
# warm read) is reported twice.
require "net/http"; require "uri"; require "json"

def walk(base, routes)
  jar = {}; csrf = nil
  req = lambda do |verb, path, form = nil|
    uri = URI.join(base, path)
    r = verb == "GET" ? Net::HTTP::Get.new(uri) : Net::HTTP::Post.new(uri)
    r["Accept"] = "text/html"
    r["Cookie"] = jar.map { |k, v| "#{k}=#{v}" }.join("; ") unless jar.empty?
    r["X-CSRF-Token"] = csrf if verb == "POST" && csrf
    r.set_form_data(form) if form
    res = Net::HTTP.start(uri.host, uri.port) { |h| h.request(r) }
    Array(res.get_fields("set-cookie")).each { |c| k, v = c.split(";", 2)[0].split("=", 2); jar[k] = v }
    if verb == "GET" && (t = res.body.to_s[/<meta name="csrf-token" content="([^"]+)"/, 1]) then csrf = t end
    res
  end
  # The sign-in page is measured unauthenticated -- that is how a visitor
  # meets it -- and it doubles as the CSRF source for the POST.
  res = req.call("GET", "/session/new")
  puts "/session/new -> #{res.code} #{res.body.to_s.bytesize}"
  req.call("POST", "/session", { "email_address" => ENV.fetch("CAMPFIRE_EMAIL", "user1@example.com"),
                                 "password" => ENV.fetch("CAMPFIRE_PASSWORD", "secret123456") })
  abort "walk: no session cookie after POST /session" unless jar.key?("session_token")
  routes.each do |p|
    res = req.call("GET", p)
    puts "#{p} -> #{res.code} #{res.body.to_s.bytesize}"
  end
end

# Rails: pair every "Started GET" with its "Completed" by request id.
def rails(log, routes)
  started = {}; done = []
  File.foreach(log) do |line|
    if (m = line.match(/\[([0-9a-f-]{36})\] Started (\w+) "([^"?]+)/))
      started[m[1]] = [m[2], m[3]]
    elsif (m = line.match(/\[([0-9a-f-]{36})\] Completed (\d+) .*?\((\d+) queries, (\d+) cached\)/)) && started[m[1]]
      verb, path = started.delete(m[1])
      done << { verb: verb, path: path, status: m[2].to_i, queries: m[3].to_i, cached: m[4].to_i }
    end
  end
  report(done, routes) { |r| { queries: r[:queries], cached: r[:cached], real: r[:queries] - r[:cached] } }
end

# Binary: a "-- VERB /path" marker, then one "  SQL ..." line per prepare.
def binary(log, routes)
  done = []; cur = nil
  File.foreach(log) do |line|
    if (m = line.match(/^-- (\w+) (\S+)/))
      done << cur if cur
      cur = { verb: m[1], path: m[2].sub(/\?.*/, ""), sqls: [], cached: 0 }
    elsif cur && line.start_with?("  SQL ")
      cur[:sqls] << line[6..].chomp
    elsif cur && line.start_with?("  CACHE ")
      cur[:cached] += 1
    end
  end
  done << cur if cur
  report(done, routes) do |r|
    shapes = r[:sqls].map { |s| s.gsub(/'[^']*'/, "'?'").gsub(/\b\d+\b/, "?").gsub(/\(\?(, \?)+\)/, "(?...)") }
                     .tally.sort_by { |s, n| [-n, s] }
    $stderr.puts "#{r[:path]}: #{r[:sqls].size} round trips, #{r[:cached]} replayed from the query cache"
    shapes.first(12).each { |s, n| $stderr.puts format("  %4d  %s", n, s[0, 150]) }
    { real: r[:sqls].size, cached: r[:cached] }
  end
end

# One JSON line per measured route, in walk order, consuming the log's
# requests in order so a repeated route reads its own request each time.
def report(done, routes)
  pos = 0
  routes.each do |path|
    idx = (pos...done.size).find { |i| done[i][:verb] == "GET" && done[i][:path] == path }
    if idx.nil?
      puts({ path: path, missing: true }.to_json)
      next
    end
    pos = idx + 1
    puts({ path: path, status: done[idx][:status] }.merge(yield(done[idx])).to_json)
  end
end

mode, target, *routes = ARGV
case mode
when "walk"   then walk(target, routes)
when "rails"  then rails(target, ["/session/new"] + routes)
when "binary" then binary(target, ["/session/new"] + routes)
else abort "usage: campfire-queries-drive.rb walk|rails|binary TARGET ROUTE..."
end
