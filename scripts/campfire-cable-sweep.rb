#!/usr/bin/env ruby
# scripts/campfire-cable-sweep.rb — N Action Cable sockets on ONE campfire
# server, and two curves against N: what idle sockets cost, and what a
# broadcast costs per subscriber.
#
# The driver is a plain RFC 6455 client over TCPSocket (stdlib only, so it
# runs on the bench box's system Ruby), pumped by one IO.select loop, the
# same shape the CRuby cable reactor has. Every socket signs in as a seeded
# user, loads its room page to read the <turbo-cable-stream-source> the app
# rendered (the stream name is the app's, never assumed — the August k6
# harness subscribed to names our emit does not use and counted zero
# deliveries as success), subscribes, and then:
#
#   1. connect storm  — time until all N are subscribed, and how many failed
#   2. idle           — hold for --idle seconds; server CPU and RSS, pings seen
#   3. chat           — POST --messages messages at --rate/s into the rooms,
#                       round-robin, from the seeded users; every frame that
#                       carries the message's marker is timestamped on
#                       receipt, so fan-out latency is POST-sent -> frame-read
#                       on one clock, and delivered/expected is exact.
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
         idle: 20, messages: 30, rate: 2.0, drain: 5, pid: nil, json: nil, quiet: false }
OptionParser.new do |o|
  o.on("--base URL")       { |v| opts[:base] = v }
  o.on("--sockets N", Integer) { |v| opts[:sockets] = v }
  o.on("--rooms LIST")     { |v| opts[:rooms] = v }
  o.on("--users N", Integer) { |v| opts[:users] = v }
  o.on("--idle S", Float)  { |v| opts[:idle] = v }
  o.on("--messages N", Integer) { |v| opts[:messages] = v }
  o.on("--rate R", Float)  { |v| opts[:rate] = v }
  o.on("--drain S", Float) { |v| opts[:drain] = v }
  o.on("--pid PID", Integer) { |v| opts[:pid] = v }
  o.on("--json FILE")      { |v| opts[:json] = v }
  o.on("--quiet")          { opts[:quiet] = true }
end.parse!

BASE = URI(opts[:base])
HOST = BASE.host
PORT = BASE.port
ROOMS = opts[:rooms].split(",").map(&:to_i)
N = opts[:sockets]
U = [opts[:users], N].min
RUN = SecureRandom.hex(3)

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
def log(msg) = ($stderr.puts(msg) unless $quiet)
$quiet = opts[:quiet]

# ── server sampling (Linux /proc; nil elsewhere) ─────────────────────
def cpu_ticks(pid)
  return nil unless pid && File.exist?("/proc/#{pid}/stat")
  f = File.read("/proc/#{pid}/stat").split(") ").last.split
  f[11].to_i + f[12].to_i     # utime + stime, clock ticks (100/s)
end
def rss_mb(pid)
  return nil unless pid
  if File.exist?("/proc/#{pid}/status")
    File.read("/proc/#{pid}/status")[/VmRSS:\s+(\d+)/, 1].to_i / 1024
  else
    (`ps -o rss= -p #{pid}`.to_i / 1024 rescue nil)
  end
end
def threads_of(pid)
  return nil unless pid && File.exist?("/proc/#{pid}/status")
  File.read("/proc/#{pid}/status")[/Threads:\s+(\d+)/, 1].to_i
end
TICK = 100.0

# ── the HTTP half: sessions and the page that names the stream ───────
def http(verb, path, form: nil, cookie: nil, csrf: nil, accept: "text/html")
  req = verb == :get ? Net::HTTP::Get.new(path) : Net::HTTP::Post.new(path)
  req["Accept"] = accept
  req["Cookie"] = cookie if cookie
  req["X-CSRF-Token"] = csrf if csrf
  req.set_form_data(form) if form
  Net::HTTP.start(HOST, PORT, read_timeout: 30) { |h| h.request(req) }
end

log "==> sign in #{U} users"
cookies = []
(1..U).each do |i|
  res = http(:post, "/session", form: { "email_address" => "user#{i}@example.com", "password" => "secret123456" })
  abort "sign-in for user#{i} answered #{res.code}" unless res.code == "302"
  tok = Array(res.get_fields("set-cookie")).map { |c| c.split(";", 2)[0] }.find { |c| c.start_with?("session_token=") }
  abort "no session_token for user#{i}" unless tok
  cookies << tok
end

log "==> read the #{ROOMS.size} room pages"
streams = {}   # room => [channel, signed, csrf]
ROOMS.each do |r|
  res = http(:get, "/rooms/#{r}", cookie: cookies[0])
  abort "GET /rooms/#{r} answered #{res.code}" unless res.code == "200"
  tag = res.body.to_s[/<turbo-cable-stream-source[^>]*>/]
  abort "no <turbo-cable-stream-source> on /rooms/#{r}" unless tag
  streams[r] = [tag[/channel="([^"]+)"/, 1], tag[/signed-stream-name="([^"]+)"/, 1],
                res.body.to_s[/<meta name="csrf-token" content="([^"]+)"/, 1]]
end

# ── the socket half: a hand-rolled Action Cable client ───────────────
class Cable
  attr_reader :id, :room, :state, :sock, :pings, :hits, :opened_at, :subscribed_at
  def initialize(id, room, cookie, identifier)
    @id, @room, @cookie, @identifier = id, room, cookie, identifier
    @state = :new; @buf = +""; @pings = 0; @hits = []   # hits: [t, marker]
  end
  def open!
    @opened_at = now
    @sock = TCPSocket.new(HOST, PORT)
    @sock.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
    key = [SecureRandom.random_bytes(16)].pack("m0")
    @sock.write("GET /cable HTTP/1.1\r\nHost: #{HOST}:#{PORT}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" \
                "Sec-WebSocket-Key: #{key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: actioncable-v1-json\r\n" \
                "Origin: http://#{HOST}:#{PORT}\r\nCookie: #{@cookie}\r\n\r\n")
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

  def text(payload)
    t = now
    msg = JSON.parse(payload) rescue return
    case msg["type"]
    when "welcome"
      @state = :welcomed
      send_frame(1, JSON.generate({ "command" => "subscribe", "identifier" => @identifier }))
    when "confirm_subscription"
      @state = :subscribed; @subscribed_at = t
    when "reject_subscription" then fail!("rejected")
    when "ping" then @pings += 1
    when nil
      m = msg["message"].to_s[/sweep-#{RUN}-(\d+)/, 1]
      @hits << [t, m.to_i] if m
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
  channel, signed, = streams[room]
  Cable.new(i, room, cookies[i % U], JSON.generate({ "channel" => channel, "signed_stream_name" => signed }))
end

def pump(clients, seconds)
  deadline = now + seconds
  live = clients.reject(&:closed?)
  while (left = deadline - now) > 0
    socks = live.map(&:sock)
    ready, = IO.select(socks, nil, nil, [left, 0.05].min)
    next unless ready
    ready.each do |s|
      c = live.find { |x| x.sock.equal?(s) }
      c.readable!
    end
    live = clients.reject(&:closed?) if ready.any? { |s| live.find { |x| x.sock.equal?(s) }.closed? }
    yield if block_given?
  end
end

# ── 1. connect storm ─────────────────────────────────────────────────
log "==> connect #{N} sockets"
t_storm = now
clients.each(&:open!)
pump(clients, 30) { break if clients.all? { |c| c.state == :subscribed || c.closed? } }
storm_s = now - t_storm
subscribed = clients.count { |c| c.state == :subscribed }
failed = clients.count(&:closed?)
log "   subscribed #{subscribed}/#{N} in #{storm_s.round(2)}s (#{failed} failed#{failed > 0 ? ": " + clients.select(&:closed?).map(&:why).tally.inspect : ""})"

# ── 2. idle ───────────────────────────────────────────────────────────
log "==> idle #{opts[:idle]}s"
pings0 = clients.sum(&:pings)
c0 = cpu_ticks(opts[:pid]); t0 = now
pump(clients, opts[:idle])
idle_s = now - t0
idle_cpu = c0 && cpu_ticks(opts[:pid]) ? (cpu_ticks(opts[:pid]) - c0) / TICK / idle_s : nil
idle_rss = rss_mb(opts[:pid])
idle_pings = clients.sum(&:pings) - pings0
log "   server cpu #{idle_cpu ? (idle_cpu.round(3).to_s + " cores") : "n/a"}, rss #{idle_rss || "n/a"} MB, #{idle_pings} pings in #{idle_s.round(1)}s"

# ── 3. chat ───────────────────────────────────────────────────────────
log "==> chat: #{opts[:messages]} messages at #{opts[:rate]}/s"
posts = {}
subs_in = Hash.new(0)
clients.each { |c| subs_in[c.room] += 1 if c.state == :subscribed }
c1 = cpu_ticks(opts[:pid]); t1 = now
writer = Thread.new do
  (1..opts[:messages]).each do |k|
    room = ROOMS[k % ROOMS.size]
    ck = cookies[k % U]
    sent = now
    res = begin
      http(:post, "/rooms/#{room}/messages",
           form: { "message[body]" => "sweep-#{RUN}-#{k} at #{Time.now.utc.iso8601(3)}", "message[client_message_id]" => "sweep-#{RUN}-#{k}" },
           cookie: ck, csrf: streams[room][2], accept: "text/vnd.turbo-stream.html, text/html")
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

clients.each(&:close!)
out = {
  sockets: N, rooms: ROOMS.size, users: U, subscribers_per_room: subs_in.values.max,
  storm: { seconds: storm_s.round(3), subscribed: subscribed, failed: failed },
  idle: { seconds: idle_s.round(1), cpu_cores: idle_cpu&.round(4), rss_mb: idle_rss, pings: idle_pings },
  chat: { messages: posts.size, rate: opts[:rate], non_200: bad_posts, codes: codes, delivered: delivered, expected: expected,
          frame_p50_ms: pct.(lat, 0.5), frame_p99_ms: pct.(lat, 0.99), last_p50_ms: pct.(last, 0.5), last_p99_ms: pct.(last, 0.99),
          cpu_cores: chat_cpu ? (chat_cpu / chat_s).round(4) : nil, us_per_frame: us_per_frame, rss_mb: chat_rss },
  server_threads: threads_of(opts[:pid])
}
File.write(opts[:json], JSON.generate(out) + "\n") if opts[:json]
puts JSON.generate(out) if opts[:quiet]
