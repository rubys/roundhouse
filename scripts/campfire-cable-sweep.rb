#!/usr/bin/env ruby
# scripts/campfire-cable-sweep.rb — N Action Cable sockets on ONE campfire
# server, and two curves against N: what idle sockets cost, and what a
# broadcast costs per subscriber.
#
# The driver is a plain RFC 6455 client over TCPSocket (stdlib only, so it
# runs on the bench box's system Ruby), pumped by one IO.select loop, the
# same shape the CRuby cable reactor has. It knows nothing about which
# runtime answers the port — that is what lets the SAME script drive the
# campfire binary and Rails as ONCE configures it, and what makes the two
# results comparable.
#
# Every socket signs in as a seeded user, loads its room page to read the
# <turbo-cable-stream-source> the app rendered (the stream name is the
# app's, never assumed — the August k6 harness subscribed to names our
# emit does not use and counted zero deliveries as success), opens ONE
# WebSocket carrying the subscriptions a real campfire tab carries, and
# then:
#
#   1. connect storm  — time until all N are subscribed, and how many failed
#   2. idle           — hold for --idle seconds; server CPU and RSS, pings seen
#   3. chat           — POST --messages messages at --rate/s into the rooms,
#                       round-robin, from the seeded users; every frame that
#                       carries the message's marker is timestamped on
#                       receipt, so fan-out latency is POST-sent -> frame-read
#                       on one clock, and delivered/expected is exact.
#   4. teardown       — close all N at once and hold --settle seconds; the
#                       disconnect storm's server cost.
#
# TWO SUBSCRIPTIONS PER SOCKET, because that is what the browser opens.
# campfire's room page mounts a `<turbo-cable-stream-source
# channel="RoomMessagesChannel">` AND `presence_controller.js` subscribes
# to `{"channel":"PresenceChannel","room_id":<id>}` on the same consumer.
# Only the second one WRITES: `PresenceChannel`'s `on_subscribe :present`
# runs `membership.present`, an UPDATE per subscribe, and its
# `on_unsubscribe :absent` another on close. A sweep that opened the
# message stream alone measured a connect storm with no database in it —
# which is the one thing once.com/campfire's requirements table is most
# likely to be sized by. `--no-presence` restores the older shape, and
# every result records which one it was.
#
# SIGN-IN GOES THROUGH THE FORM, WITH THE TOKEN. Rails campfire's
# `Authentication` concern is `protect_from_forgery with: :exception`, so
# a POST needs both the session cookie and the `authenticity_token` the
# page carried; the binary does not check, but a driver that skipped the
# token would only ever have run against the lane that forgives it. Every
# response's Set-Cookie is absorbed into a per-user jar, and the POSTs in
# the chat phase carry THAT user's `csrf-token` meta, not user 1's.
#
# AND IT DOES NOT SCALE, WHICH IS WHY --cookies EXISTS. campfire rate-limits
# `SessionsController#create` to 10 per 3 minutes per IP (MEASURED against
# the oracle: sign-in 8 answers 429), and each one is a bcrypt at ~100 ms.
# Signing in a thousand users is not a slow setup, it is an impossible
# one. `--cookies FILE` takes one signed `session_token` per line — user
# i on line i — and skips sign-in entirely, which is exactly what ONCE's
# own load harness does (`test/performance/create_dummy_cookies.rb` ships
# 10,000 pre-forged cookies for the same reason). Mint the file with
# `scripts/campfire-oracle cookies`; the same file works on BOTH lanes,
# because both verify a Rails `_rails`-envelope signed cookie under the
# same SECRET_KEY_BASE.
#
# Prints one human line and, with --json, one JSON object.
require "socket"
require "time"
require "json"
require "net/http"
require "uri"
require "securerandom"
require "optparse"

opts = { base: "http://127.0.0.1:3000", sockets: 10, rooms: "1,2,3,4,5", users: 50,
         idle: 20, messages: 30, rate: 2.0, drain: 5, settle: 5, presence: true,
         cookies: nil, pid: nil, json: nil, quiet: false }
OptionParser.new do |o|
  o.on("--base URL")       { |v| opts[:base] = v }
  o.on("--sockets N", Integer) { |v| opts[:sockets] = v }
  o.on("--rooms LIST")     { |v| opts[:rooms] = v }
  o.on("--users N", Integer) { |v| opts[:users] = v }
  o.on("--idle S", Float)  { |v| opts[:idle] = v }
  o.on("--messages N", Integer) { |v| opts[:messages] = v }
  o.on("--rate R", Float)  { |v| opts[:rate] = v }
  o.on("--drain S", Float) { |v| opts[:drain] = v }
  o.on("--pace S", Float)  { |v| opts[:pace] = v }
  o.on("--settle S", Float) { |v| opts[:settle] = v }
  o.on("--[no-]presence")  { |v| opts[:presence] = v }
  o.on("--cookies FILE")   { |v| opts[:cookies] = v }
  o.on("--pid PID", Integer) { |v| opts[:pid] = v }
  o.on("--json FILE")      { |v| opts[:json] = v }
  o.on("--quiet")          { opts[:quiet] = true }
end.parse!

BASE = URI(opts[:base])
HOST = BASE.host
PORT = BASE.port
ROOMS = opts[:rooms].split(",").map(&:to_i)
N = opts[:sockets]
RUN = SecureRandom.hex(3)

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
def log(msg) = ($stderr.puts(msg) unless $quiet)
$quiet = opts[:quiet]

# ── server sampling (Linux /proc; nil elsewhere) ─────────────────────
#
# THE WHOLE PROCESS TREE, not the pid handed in. The binary is one
# process with N OS worker threads inside it, so a single /proc read was
# the whole server; clustered Puma is a master and N forked workers, and
# reading the master alone would have charged Rails almost nothing for
# almost everything it did. The tree is rediscovered on every sample —
# Puma replaces a worker it reaps, and a sample that cached the pid list
# would silently stop counting it.
def tree_pids(pid)
  return [] unless pid
  return [pid] unless File.directory?("/proc")
  children = Hash.new { |h, k| h[k] = [] }
  Dir.glob("/proc/[0-9]*/stat").each do |path|
    stat = (File.read(path) rescue next).split(") ").last.split
    children[stat[1].to_i] << File.basename(File.dirname(path)).to_i
  end
  out = [pid]
  queue = [pid]
  until queue.empty?
    kids = children[queue.shift]
    out.concat(kids); queue.concat(kids)
  end
  out.uniq
end

def cpu_ticks(pid)
  return nil unless pid && File.exist?("/proc/#{pid}/stat")
  tree_pids(pid).sum do |p|
    f = (File.read("/proc/#{p}/stat") rescue next 0).split(") ").last.split
    f[11].to_i + f[12].to_i   # utime + stime, clock ticks (100/s)
  end
end

# RSS summed over the tree, and PSS beside it. They are the same number
# for a one-process server and they are NOT for a forked one: every
# worker's copy of a shared page is counted once per worker in RSS and
# split between them in PSS. Publishing RSS alone would hand the
# clustered lane a penalty it does not really pay; publishing PSS alone
# would break comparison with the single-process table already measured.
# Both, and the report says which is which.
def rss_mb(pid)
  return nil unless pid
  if File.directory?("/proc")
    kb = tree_pids(pid).sum { |p| (File.read("/proc/#{p}/status")[/VmRSS:\s+(\d+)/, 1].to_i rescue 0) }
    kb.zero? ? nil : kb / 1024
  else
    (`ps -o rss= -p #{pid}`.to_i / 1024 rescue nil)
  end
end

def pss_mb(pid)
  return nil unless pid && File.exist?("/proc/#{pid}/smaps_rollup")
  kb = tree_pids(pid).sum { |p| (File.read("/proc/#{p}/smaps_rollup")[/^Pss:\s+(\d+)/, 1].to_i rescue 0) }
  kb.zero? ? nil : kb / 1024
end

def threads_of(pid)
  return nil unless pid && File.exist?("/proc/#{pid}/status")
  tree_pids(pid).sum { |p| (File.read("/proc/#{p}/status")[/Threads:\s+(\d+)/, 1].to_i rescue 0) }
end

def procs_of(pid) = pid ? tree_pids(pid).size : nil
TICK = 100.0

# ── the HTTP half: a cookie jar per user, and the pages it reads ─────
#
# A JAR, not the one `session_token=` line the older driver kept. Rails
# hands out TWO cookies — `session_token` (who you are) and
# `_campfire_session` (which the CSRF token is bound to) — and it rotates
# the second one; posting with the first alone is a 422 on that lane and
# a silent pass on ours.
class Jar
  def initialize(pairs = {}) = @c = pairs
  def absorb(res)
    Array(res.get_fields("set-cookie")).each do |line|
      name, value = line.split(";", 2)[0].split("=", 2)
      @c[name.strip] = value if name && value
    end
    self
  end
  def [](name) = @c[name]
  def to_s = @c.map { |k, v| "#{k}=#{v}" }.join("; ")
  def empty? = @c.empty?
end

def http(verb, path, form: nil, jar: nil, csrf: nil, accept: "text/html")
  req = verb == :get ? Net::HTTP::Get.new(path) : Net::HTTP::Post.new(path)
  req["Accept"] = accept
  req["Cookie"] = jar.to_s if jar && !jar.empty?
  req["X-CSRF-Token"] = csrf if csrf
  req.set_form_data(form) if form
  res = Net::HTTP.start(HOST, PORT, read_timeout: 30) { |h| h.request(req) }
  jar&.absorb(res)
  res
end

# The token a Rails form carries. Absent on the binary, which does not
# check — hence `to_s` at every call site rather than an abort.
def form_token(body) = body.to_s[/name="authenticity_token"[^>]*value="([^"]*)"/, 1]
def meta_token(body) = body.to_s[/<meta name="csrf-token" content="([^"]+)"/, 1]

if opts[:cookies]
  lines = File.readlines(opts[:cookies], chomp: true).reject(&:empty?)
  U = [opts[:users], N, lines.size].min
  log "==> #{U} pre-minted cookies from #{opts[:cookies]} (no sign-in; see the header)"
  JARS = (0...U).map { |i| Jar.new({ "session_token" => lines[i].sub(/\Asession_token=/, "") }) }
else
  U = [opts[:users], N].min
  log "==> sign in #{U} users through the form"
  JARS = (1..U).map do |i|
    jar = Jar.new
    page = http(:get, "/session/new", jar: jar)
    abort "GET /session/new answered #{page.code}" unless page.code == "200"
    res = http(:post, "/session", jar: jar,
               form: { "email_address" => "user#{i}@example.com", "password" => "secret123456",
                       "authenticity_token" => form_token(page.body).to_s })
    abort "sign-in for user#{i} answered #{res.code}#{res.code == "429" ? " — rate limited; use --cookies (see the header)" : ""}" unless res.code == "302"
    abort "no session_token for user#{i}" unless jar["session_token"]
    jar
  end
end

# ── the pages: the stream each room names, and each writer's token ───
#
# One page per ROOM for the signed stream name (it is signed per room,
# not per user), and one page per WRITER for that user's CSRF token —
# not per socket. A thousand room-page renders before the storm would
# be setup cost larger than the measurement.
log "==> read the #{ROOMS.size} room pages"
streams = {}                      # room => [channel, signed-stream-name]
ROOMS.each do |r|
  res = http(:get, "/rooms/#{r}", jar: JARS[0])
  abort "GET /rooms/#{r} answered #{res.code}" unless res.code == "200"
  tag = res.body.to_s[/<turbo-cable-stream-source[^>]*>/]
  abort "no <turbo-cable-stream-source> on /rooms/#{r}" unless tag
  streams[r] = [tag[/channel="([^"]+)"/, 1], tag[/signed-stream-name="([^"]+)"/, 1]]
end

writers = (1..opts[:messages]).map { |k| k % U }.uniq
csrf = { 0 => meta_token(http(:get, "/rooms/#{ROOMS[0]}", jar: JARS[0]).body) }
(writers - [0]).each do |u|
  csrf[u] = meta_token(http(:get, "/rooms/#{ROOMS[u % ROOMS.size]}", jar: JARS[u]).body)
end
log "   #{writers.size} writer token(s)#{csrf[writers[0]] ? "" : " — none carried a csrf-token meta (the binary does not check)"}"

# ── the socket half: a hand-rolled Action Cable client ───────────────
class Cable
  attr_reader :id, :room, :state, :sock, :pings, :hits, :opened_at, :subscribed_at, :confirmed, :rejected
  def initialize(id, room, jar, identifiers)
    @id, @room, @jar, @identifiers = id, room, jar, identifiers
    @state = :new; @buf = +""; @pings = 0; @hits = []   # hits: [t, marker]
    @confirmed = 0; @rejected = 0
  end
  def open!
    @opened_at = now
    @sock = TCPSocket.new(HOST, PORT)
    @sock.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
    key = [SecureRandom.random_bytes(16)].pack("m0")
    @sock.write("GET /cable HTTP/1.1\r\nHost: #{HOST}:#{PORT}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" \
                "Sec-WebSocket-Key: #{key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: actioncable-v1-json\r\n" \
                "Origin: http://#{HOST}:#{PORT}\r\nCookie: #{@jar}\r\n\r\n")
    @state = :handshake
  end
  def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  def closed? = @state == :closed || @state == :failed
  def fail!(why) = (@state = :failed; @why = why; (@sock.close rescue nil))
  def close! = ((@sock.close rescue nil); @state = :closed)
  def why = @why

  def readable!
    data = @sock.read_nonblock(65536, exception: false)
    return if data == :wait_readable
    return fail!("eof in #{@state}") if data.nil?
    @buf << data
    if @state == :handshake
      i = @buf.index("\r\n\r\n") or return
      head = @buf[0, i]
      return fail!("handshake: #{head.lines.first.to_s.strip}") unless head.start_with?("HTTP/1.1 101")
      @buf = @buf[(i + 4)..]
      @state = :open
    end
    parse_frames
  end

  def parse_frames
    loop do
      return if @buf.bytesize < 2
      b0 = @buf.getbyte(0); b1 = @buf.getbyte(1)
      op = b0 & 0x0f; len = b1 & 0x7f; off = 2
      if len == 126
        return if @buf.bytesize < 4
        len = @buf.byteslice(2, 2).unpack1("n"); off = 4
      elsif len == 127
        return if @buf.bytesize < 10
        len = @buf.byteslice(2, 8).unpack1("Q>"); off = 10
      end
      off += 4 if (b1 & 0x80) != 0   # a masked server frame is illegal, but skip the key
      return if @buf.bytesize < off + len
      payload = @buf.byteslice(off, len)
      @buf = @buf.byteslice(off + len, @buf.bytesize - off - len)
      case op
      when 1 then text(payload)
      when 8 then @state = :closed; return
      when 9 then send_frame(10, payload)
      end
    end
  end

  # ONE `welcome` and then a subscribe frame per identifier, on the one
  # socket — an Action Cable consumer multiplexes its subscriptions, and
  # opening a second socket for presence would double the descriptor
  # count the sweep is measuring.
  def text(payload)
    t = now
    msg = JSON.parse(payload) rescue return
    case msg["type"]
    when "welcome"
      @state = :welcomed
      @identifiers.each { |ident| send_frame(1, JSON.generate({ "command" => "subscribe", "identifier" => ident })) }
    when "confirm_subscription"
      @confirmed += 1
      settled!(t)
    when "reject_subscription"
      @rejected += 1
      settled!(t)
    when "ping" then @pings += 1
    when nil
      m = msg["message"].to_s[/sweep-#{RUN}-(\d+)/, 1]
      @hits << [t, m.to_i] if m
    end
  end

  # Subscribed once every subscription has been answered and at least one
  # was confirmed. A rejected presence subscription must not take the
  # message stream down with it — and it must not be counted as success.
  def settled!(t)
    return unless @confirmed + @rejected >= @identifiers.size
    if @confirmed > 0
      @state = :subscribed; @subscribed_at = t
    else
      fail!("all #{@identifiers.size} subscriptions rejected")
    end
  end

  def send_frame(op, payload)
    mask = SecureRandom.random_bytes(4)
    len = payload.bytesize
    head = [0x80 | op].pack("C")
    head << (len < 126 ? [0x80 | len].pack("C") : len < 65536 ? [0x80 | 126, len].pack("Cn") : [0x80 | 127, len].pack("CQ>"))
    masked = payload.bytes.each_with_index.map { |b, i| b ^ mask.getbyte(i % 4) }.pack("C*")
    @sock.write(head + mask + masked)
  rescue IOError, SystemCallError => e
    fail!("write: #{e.class}")
  end
end

clients = (0...N).map do |i|
  room = ROOMS[i % ROOMS.size]
  channel, signed = streams[room]
  idents = [JSON.generate({ "channel" => channel, "signed_stream_name" => signed })]
  # room_id as a NUMBER, the way `cable.subscribeTo({channel:
  # "PresenceChannel", room_id: Current.room.id})` serializes it.
  idents << JSON.generate({ "channel" => "PresenceChannel", "room_id" => room }) if opts[:presence]
  Cable.new(i, room, JARS[i % U], idents)
end
subs_per_socket = clients[0].nil? ? 0 : 1 + (opts[:presence] ? 1 : 0)

# THE DRIVER IS ONE THREAD, and above a few thousand sockets that is a
# measurement instrument with a capacity of its own. A server that beats
# every connection from one heartbeat delivers N ping frames in a burst,
# and this loop has to read and parse all of them before it can stamp
# the next message frame — which would show up as the SERVER's tail
# latency and is not. `$drain_s` / `$drain_max` price that: total time
# spent reading frames, and the longest single pass. Reset per phase and
# reported with the chat window.
$drain_s = 0.0
$drain_max = 0.0

# A paced storm calls this between opens, so some clients have not been
# opened yet and have no socket; `IO.select` on a nil is a TypeError.
def readable_clients(clients)
  clients.reject { |c| c.closed? || c.sock.nil? }
end

def pump(clients, seconds)
  deadline = now + seconds
  live = readable_clients(clients)
  while (left = deadline - now) > 0
    socks = live.map(&:sock)
    if socks.empty?
      sleep([left, 0.05].min)
      yield if block_given?
      next
    end
    ready, = IO.select(socks, nil, nil, [left, 0.05].min)
    next unless ready
    t_pass = now
    ready.each do |s|
      c = live.find { |x| x.sock.equal?(s) }
      c.readable!
    end
    pass = now - t_pass
    $drain_s += pass
    $drain_max = pass if pass > $drain_max
    live = readable_clients(clients) if ready.any? { |s| live.find { |x| x.sock.equal?(s) }.closed? }
    yield if block_given?
  end
end

# ── 1. connect storm ─────────────────────────────────────────────────
log "==> connect #{N} sockets (#{subs_per_socket} subscription#{subs_per_socket == 1 ? "" : "s"} each#{opts[:presence] ? ", presence writes on" : ""}#{opts[:pace] ? ", paced #{opts[:pace]}s apart" : ""})"
storm_c0 = cpu_ticks(opts[:pid])
t_storm = now
# HOW FAST THE CONNECTIONS ARRIVE IS A MEASUREMENT AXIS, not an artifact.
# A storm that opens everything at once leaves the per-connection ping
# phases clustered, one scheduler wake serves the whole beat, and the
# idle cost that comes back is the best case; connections in a
# deployment arrive over minutes and spread the phases fully. `--pace`
# puts the arrivals at a chosen interval, and pumps between them so the
# already-open sockets are read while the rest are still arriving.
if opts[:pace] && opts[:pace] > 0
  clients.each do |c|
    c.open!
    pump(clients, opts[:pace])
  end
else
  clients.each(&:open!)
end
pump(clients, 60) { break if clients.all? { |c| c.state == :subscribed || c.closed? } }
storm_s = now - t_storm
storm_cpu = storm_c0 && cpu_ticks(opts[:pid]) ? (cpu_ticks(opts[:pid]) - storm_c0) / TICK : nil
subscribed = clients.count { |c| c.state == :subscribed }
failed = clients.count(&:closed?)
confirmations = clients.sum(&:confirmed)
rejections = clients.sum(&:rejected)
log "   subscribed #{subscribed}/#{N} in #{storm_s.round(2)}s (#{confirmations} confirmations, #{rejections} rejections, #{failed} failed#{failed > 0 ? ": " + clients.select(&:closed?).map(&:why).tally.inspect : ""}); server cpu #{storm_cpu ? storm_cpu.round(2).to_s + "s" : "n/a"}"

# ── 2. idle ───────────────────────────────────────────────────────────
log "==> idle #{opts[:idle]}s"
pings0 = clients.sum(&:pings)
c0 = cpu_ticks(opts[:pid]); t0 = now
pump(clients, opts[:idle])
idle_s = now - t0
idle_cpu = c0 && cpu_ticks(opts[:pid]) ? (cpu_ticks(opts[:pid]) - c0) / TICK / idle_s : nil
idle_rss = rss_mb(opts[:pid])
idle_pss = pss_mb(opts[:pid])
idle_pings = clients.sum(&:pings) - pings0
log "   server cpu #{idle_cpu ? (idle_cpu.round(3).to_s + " cores") : "n/a"}, rss #{idle_rss || "n/a"} MB#{idle_pss ? " (pss #{idle_pss} MB over #{procs_of(opts[:pid])} processes)" : ""}, #{idle_pings} pings in #{idle_s.round(1)}s"

# ── 3. chat ───────────────────────────────────────────────────────────
log "==> chat: #{opts[:messages]} messages at #{opts[:rate]}/s"
posts = {}
subs_in = Hash.new(0)
clients.each { |c| subs_in[c.room] += 1 if c.state == :subscribed }
c1 = cpu_ticks(opts[:pid]); t1 = now
$drain_s = 0.0; $drain_max = 0.0
writer = Thread.new do
  (1..opts[:messages]).each do |k|
    room = ROOMS[k % ROOMS.size]
    u = k % U
    sent = now
    res = begin
      http(:post, "/rooms/#{room}/messages",
           form: { "message[body]" => "sweep-#{RUN}-#{k} at #{Time.now.utc.iso8601(3)}", "message[client_message_id]" => "sweep-#{RUN}-#{k}" },
           jar: JARS[u], csrf: csrf[u], accept: "text/vnd.turbo-stream.html, text/html")
    rescue => e
      e
    end
    posts[k] = { room: room, sent: sent, done: now, code: res.respond_to?(:code) ? res.code : res.class.name,
                 note: res.respond_to?(:code) && res.code != "200" ? (res["Location"] || res.body.to_s[0, 160]).to_s : (res.is_a?(Exception) ? res.message[0, 160] : nil) }
    sleep(1.0 / opts[:rate])
  end
end
# Read until the writer has posted everything AND --drain seconds have passed
# since its last POST. The window is not an estimate: a POST into a big room
# takes as long as its fan-out, and a fixed budget stopped reading before the
# last messages of the 5,000-socket cell were even posted.
last_post = now
pump(clients, opts[:messages] / opts[:rate] + opts[:drain] + 600) do
  last_post = posts.values.map { |p| p[:done] }.max || last_post
  break if !writer.alive? && now - last_post > opts[:drain]
end
writer.join
chat_s = now - t1
chat_cpu = c1 && cpu_ticks(opts[:pid]) ? (cpu_ticks(opts[:pid]) - c1) / TICK : nil
chat_rss = rss_mb(opts[:pid])

lat = []; last = []; delivered = 0; expected = 0
posts.each do |k, p|
  hits = clients.flat_map { |c| c.hits.select { |_, m| m == k }.map { |t, _| t } }
  expected += subs_in[p[:room]]
  delivered += hits.size
  hits.each { |t| lat << (t - p[:sent]) * 1000 }
  last << (hits.max - p[:sent]) * 1000 if hits.any?
end
pct = ->(a, q) { a.empty? ? nil : a.sort[[(a.size * q).ceil - 1, 0].max].round(2) }
bad_posts = posts.values.count { |p| p[:code] != "200" }
codes = posts.values.map { |p| p[:code] }.tally
log "   post codes: #{codes.inspect}#{bad_posts > 0 ? " — first failure: " + posts.values.find { |p| p[:code] != "200" }[:note].to_s.inspect : ""}" if bad_posts > 0
us_per_frame = chat_cpu && delivered > 0 ? (chat_cpu * 1e6 / delivered).round(1) : nil
log "   delivered #{delivered}/#{expected} frames from #{posts.size} posts (#{bad_posts} non-200); frame p50 #{pct.(lat, 0.5)} ms p99 #{pct.(lat, 0.99)} ms; last-frame p50 #{pct.(last, 0.5)} p99 #{pct.(last, 0.99)} ms; server cpu #{chat_cpu ? (chat_cpu / chat_s).round(3) : "n/a"} cores, #{us_per_frame || "n/a"} us/frame, rss #{chat_rss || "n/a"} MB"
# WHAT THE DRIVER ITSELF COST during that window. A latency here is a
# server number only to the extent this thread was free to stamp it:
# `drain max` is the longest single reading pass, and if it is the size
# of the p99 above then the p99 is the driver's backlog, not the
# server's fan-out.
log "   driver: #{(100 * $drain_s / chat_s).round(1)}% of the window spent reading frames, longest pass #{($drain_max * 1000).round(1)} ms"

# ── 4. teardown ───────────────────────────────────────────────────────
#
# The disconnect storm, and with presence on it is the other half of the
# write path: every socket's `on_unsubscribe :absent` is a row. Nothing
# client-side observes when the server has finished, so the window is
# fixed and the reading is the server's CPU inside it — a floor, not a
# duration.
log "==> teardown: close #{clients.count { |c| !c.closed? }} sockets, settle #{opts[:settle]}s"
d0 = cpu_ticks(opts[:pid]); t_down = now
clients.each(&:close!)
sleep(opts[:settle])
down_s = now - t_down
down_cpu = d0 && cpu_ticks(opts[:pid]) ? (cpu_ticks(opts[:pid]) - d0) / TICK : nil
down_rss = rss_mb(opts[:pid])
log "   server cpu #{down_cpu ? down_cpu.round(2).to_s + "s over " + down_s.round(1).to_s + "s" : "n/a"}, rss #{down_rss || "n/a"} MB"

out = {
  sockets: N, rooms: ROOMS.size, users: U, subscribers_per_room: subs_in.values.max,
  presence: opts[:presence], subscriptions_per_socket: subs_per_socket,
  sign_in: opts[:cookies] ? "pre-minted cookies" : "form + csrf token",
  storm: { seconds: storm_s.round(3), subscribed: subscribed, failed: failed,
           confirmations: confirmations, rejections: rejections, cpu_seconds: storm_cpu&.round(3) },
  idle: { seconds: idle_s.round(1), cpu_cores: idle_cpu&.round(4), rss_mb: idle_rss, pss_mb: idle_pss, pings: idle_pings },
  chat: { messages: posts.size, rate: opts[:rate], non_200: bad_posts, codes: codes, delivered: delivered, expected: expected,
          frame_p50_ms: pct.(lat, 0.5), frame_p99_ms: pct.(lat, 0.99), last_p50_ms: pct.(last, 0.5), last_p99_ms: pct.(last, 0.99),
          cpu_cores: chat_cpu ? (chat_cpu / chat_s).round(4) : nil, us_per_frame: us_per_frame, rss_mb: chat_rss,
          driver_read_share: chat_s > 0 ? (100 * $drain_s / chat_s).round(1) : nil,
          driver_max_pass_ms: ($drain_max * 1000).round(1) },
  teardown: { seconds: down_s.round(1), cpu_seconds: down_cpu&.round(3), rss_mb: down_rss },
  server_threads: threads_of(opts[:pid]), server_processes: procs_of(opts[:pid])
}
File.write(opts[:json], JSON.generate(out) + "\n") if opts[:json]
puts JSON.generate(out) if opts[:quiet]
