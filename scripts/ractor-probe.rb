# Ractor-unsafety probe for a roundhouse CRuby emit of campfire.
#
#   cd EMIT && bundle exec ruby /path/to/scripts/ractor-probe.rb [trace|census|routes|all] [OUT.json]
#
# Needs a seeded storage/development.sqlite3 with user1@example.com /
# secret123456 (the campfire oracle seed). Boots the emit in the main
# Ractor the way config.ru does (no server), signs in, serves the bench's
# dynamic routes once in the main Ractor (baseline), then
#   trace:  per route, which non-app methods it reaches (TracePoint, main
#           Ractor). The room page is traced cold too: fragment caches hide
#           the rich-text path on a warm render, and under Ractors every
#           worker starts cold.
#   census: constants and module ivars a worker Ractor could not read, by
#           owner, and whether a boot-time deep freeze could fix each
#           constant; class variables.
#   routes: each route inside a fresh worker Ractor; the first error.
# C extensions are checked by calling them: scripts/ractor-probe-calls.rb.
# Feeds docs/ractor-ledger.md.

$VERBOSE = nil
require "json"
require "rack"
require "rack/mock_request"
require "stringio"
require File.expand_path("main")
require File.expand_path("cable")
require File.expand_path("runtime/gzip_cache")
Main.configure_default_adapter!

MODE = ARGV.fetch(0, "all")
OUT = ARGV.fetch(1, "ractor-probe-#{MODE}.json")
TRACE_ON = %w[trace all].include?(MODE)
EMIT = Dir.pwd
GEMS = Gem.loaded_specs.values.map { |s| [s.full_gem_path, s.name] }.sort_by { |p, _| -p.size }
STDLIB = RbConfig::CONFIG.values_at("rubylibdir", "archdir", "rubyarchdir").compact

def owner_of(path)
  return "?" if path.nil?
  return "app/#{path.delete_prefix(EMIT + "/")}" if path.start_with?(EMIT) && !path.include?("/vendor/")
  GEMS.each { |p, n| return "gem:#{n}" if path.start_with?(p) }
  return "stdlib" if STDLIB.any? { |d| path.start_with?(d) } || path.start_with?("<internal")
  path
end

def env_for(method, path, cookie: nil, params: nil, headers: {})
  opts = { method: method }
  opts["HTTP_COOKIE"] = cookie if cookie
  headers.each { |k, v| opts[k] = v }
  opts[:params] = params if params
  Rack::MockRequest.env_for(path, opts)
end

TRACE_SEEN = {}
def traced(label)
  return yield unless TRACE_ON
  seen = Hash.new(0)
  tp = TracePoint.new(:call, :c_call) do |t|
    path = t.path
    next if path == __FILE__ || (path.start_with?(EMIT) && !path.include?("/vendor/"))
    klass = t.defined_class
    kname = (klass.singleton_class? ? (klass.attached_object.inspect rescue "?") + "." : (klass.name || klass.inspect) + "#") rescue "?"
    seen["#{t.event == :c_call ? "C" : "R"} #{owner_of(path)} #{kname}#{t.method_id}"] += 1
  end
  r = tp.enable { yield }
  TRACE_SEEN[label] = seen
  r
end

def serve(method, path, cookie: nil, params: nil, headers: {}, label: nil)
  env = env_for(method, path, cookie: cookie, params: params, headers: headers)
  traced(label || "#{method} #{path}") { Db.with_connection { Main.run_rack(env) } }
end

def cookies_from(headers, jar = {})
  Array(headers["set-cookie"]).each do |line|
    k, v = line.split(";").first.split("=", 2)
    v.to_s.empty? ? jar.delete(k) : jar[k] = v
  end
  jar
end

# ── sign in, scrape, baseline (main Ractor) ──────────────────────────
jar = {}
_, h, b = serve("GET", "/session/new")
cookies_from(h, jar)
token = b.join[/name="authenticity_token" value="([^"]*)"/, 1]
st, h, = serve("POST", "/session", cookie: jar.map { |k, v| "#{k}=#{v}" }.join("; "),
               params: { "authenticity_token" => token, "email_address" => "user1@example.com", "password" => "secret123456" })
cookies_from(h, jar)
abort "sign-in failed: #{st}" unless st == 302 && h["location"].to_s !~ %r{/session/new}
COOKIE = jar.map { |k, v| "#{k}=#{v}" }.join("; ").freeze

html = serve("GET", "/rooms/1", cookie: COOKIE, label: "GET /rooms/1 (cold)")[2].join
avatar = html[%r{/users/[^/"?]+/avatar}, 0]
csrf = html[/name="csrf-token" content="([^"]*)"/, 1]
ROUTES = Ractor.make_shareable([
  ["GET", "/rooms/1"],
  ["GET", "/rooms/1/messages?before=50"],
  ["GET", "/users/me/sidebar"],
  ["GET", "/searches?q=coffee"],
  ["GET", avatar.to_s],
  ["GET", "/up"],
  ["POST", "/rooms/2/messages", { "message[body]" => "ractor probe", "authenticity_token" => csrf.to_s },
   { "HTTP_ACCEPT" => "text/vnd.turbo-stream.html, text/html" }],
])

baseline = ROUTES.map do |m, p, params, hdrs|
  [m + " " + p, serve(m, p, cookie: COOKIE, params: params, headers: hdrs || {})[0]]
end
puts "baseline (main Ractor):"
baseline.each { |r, s| puts "  #{s}  #{r[0, 60]}" }
report = { baseline: baseline }

# ── census ───────────────────────────────────────────────────────────
if %w[census all].include?(MODE)
  consts, ivars, cvars = [], [], []
  ObjectSpace.each_object(Module) do |mod|
    name = (Module.instance_method(:name).bind_call(mod) rescue nil)
    next if name.nil?
    where = lambda do
      m0 = (mod.singleton_methods(false) + mod.instance_methods(false)).first
      loc = m0 && ((mod.method(m0) rescue mod.instance_method(m0)).source_location rescue nil)
      owner_of(loc&.first)
    end
    mod.constants(false).each do |c|
      next if mod.autoload?(c)
      v = (mod.const_get(c, false) rescue next)
      next if v.is_a?(Module) || Ractor.shareable?(v)
      loc = (mod.const_source_location(c, false) rescue nil)
      freezable = begin; Ractor.make_shareable(v, copy: true); true; rescue => e; e.message[0, 120]; end
      consts << { name: "#{name}::#{c}", cls: v.class.to_s, owner: owner_of(loc&.first), loc: loc&.join(":"), freezable: freezable }
    end
    mod.instance_variables.each do |iv|
      v = mod.instance_variable_get(iv)
      ivars << { name: "#{name}.#{iv}", cls: v.class.to_s, owner: where.call } unless Ractor.shareable?(v)
    end
    (mod.class_variables(false) rescue []).each { |cv| cvars << { name: "#{name}#{cv}", owner: where.call } }
  end
  report.merge!(constants: consts, module_ivars: ivars, class_variables: cvars)
  group = ->(xs) { xs.group_by { |x| x[:owner].sub(%r{\Aapp/(runtime|app|config)/.*}, 'app/\1') }.sort_by { |_, v| -v.size } }
  puts "constants not shareable: #{consts.size}; a deep freeze cannot fix #{consts.count { |c| c[:freezable] != true }}:"
  group.(consts.reject { |c| c[:freezable] == true }).each { |o, v| puts "  #{o}: #{v.map { |c| "#{c[:name]} (#{c[:cls]})" }.join(", ")}" }
  puts "module ivars not shareable: #{ivars.size}"
  group.(ivars).each { |o, v| puts "  #{v.size.to_s.rjust(4)}  #{o}" }
  puts "class variables: #{cvars.size}"
  group.(cvars).each { |o, v| puts "  #{v.size.to_s.rjust(4)}  #{o}" }
end

if TRACE_ON
  report[:trace] = TRACE_SEEN
  puts "third-party code reached, per route (distinct methods):"
  TRACE_SEEN.each do |route, calls|
    gems = calls.keys.map { |k| k.split(" ")[1] }.reject { |o| o == "stdlib" || o == "?" }.tally
    puts "  #{route[0, 40].ljust(40)} #{gems.sort_by { |_, v| -v }.map { |g, n| "#{g.delete_prefix("gem:")}(#{n})" }.join(" ")}"
  end
end

# ── routes in a worker Ractor ────────────────────────────────────────
if %w[routes all].include?(MODE)
  report[:ractor_routes] = ROUTES.map do |m, p, params, hdrs|
    env = env_for(m, p, cookie: COOKIE, params: params&.dup, headers: hdrs || {})
    body = env.delete("rack.input")&.read.to_s
    env.delete("rack.errors")
    r = Ractor.new(Ractor.make_shareable(env, copy: true), body.freeze) do |env0, body0|
      renv = env0.dup
      renv["rack.input"] = StringIO.new(body0.dup)
      renv["rack.errors"] = $stderr
      [:ok, Db.with_connection { Main.run_rack(renv) }[0]]
    rescue Exception => e
      [:error, e.class.to_s, e.message[0, 300], (e.backtrace || [])[0, 6]]
    end
    { route: "#{m} #{p}", result: r.value }
  end
  puts "worker Ractor, first error per route:"
  report[:ractor_routes].each do |r|
    res = r[:result]
    puts "  #{r[:route][0, 40].ljust(40)} #{res[0]} #{res[1]}: #{res[2]}"
  end
end

File.write(OUT, JSON.pretty_generate(report))
puts "wrote #{OUT}"
