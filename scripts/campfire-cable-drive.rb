#!/usr/bin/env ruby
# The two-connection fan-out walk: sign in, open two /cable sockets,
# subscribe both through the app's own channel, post a message, and
# require that BOTH sockets receive the turbo-stream frame.
#
# #71 item 5 ("POST -> after_commit -> fan-out reaches a second
# connection") plus the halves of items 3 and 4 that only a real socket
# can show. `tests/overlay_cable_*.rb` drive the runtime directly and are
# the fast feedback; this drives the EMITTED TREE over TCP, which is the
# only place the pieces meet — a cookie the app signed, a stream name the
# app's own view minted, a channel the app's own initializer prepended a
# guard onto, and a fragment the app's own partial rendered.
#
# THAT DISTINCTION IS NOT ACADEMIC. `undefined method 'current_user=' for
# an instance of ApplicationCable::Connection` survived a green unit test
# and a green suite, because the unit fixture carried campfire's SOURCE —
# which has the `identified_by :current_user` line that ingest drops. The
# first real handshake found it in seconds.
#
# Usage: campfire-cable-drive.rb <base-url>
require "net/http"
require "uri"
require "json"
require "socket"
require "websocket/driver"

BASE = ARGV[0] or abort "usage: campfire-cable-drive.rb <base-url>"
URL  = URI(BASE)

# TWO KINDS OF PROBE, and the difference is what makes this script usable
# as a gate instead of permanently red.
#
# A MILESTONE probe is item 5's own claim — a message posted over HTTP
# reaches two live sockets — plus the items 3 and 4 halves it stands on.
# Those set the exit status.
#
# A LEDGER probe is something this walk measures on the way past that is
# a KNOWN, WRITTEN-DOWN gap somewhere else. It reports and it counts, and
# it does not fail the run, because a script that has been red since the
# day it was written stops being read. Every `ledger:` string names the
# entry in docs/pipeline/runtime.md that owns the gap; if you cannot name
# one, it is a milestone probe.
MILESTONE_FAILURES = []
LEDGER_FAILURES = []

# CABLE_WALK_DUMP=<dir>: write what the wire carried — the room page and
# the broadcast frames — so `scripts/campfire-compare` can diff this
# lane's artifacts against another lane's. Off by default; the walk
# asserts, the dump is for the comparator.
DUMP_DIR = ENV["CABLE_WALK_DUMP"]
def dump(name, content)
  return unless DUMP_DIR
  require "fileutils"
  FileUtils.mkdir_p(DUMP_DIR)
  File.write(File.join(DUMP_DIR, name), content.to_s)
end

def check(label, got, want, ledger: nil)
  if got == want
    puts "  \e[32mok\e[0m   #{label}"
  elsif ledger
    LEDGER_FAILURES << label
    puts "  \e[33mgap\e[0m  #{label}: got #{got.inspect}, want #{want.inspect}"
    puts "       known open — #{ledger}"
  else
    MILESTONE_FAILURES << label
    puts "  \e[31mFAIL\e[0m #{label}: got #{got.inspect}, want #{want.inspect}"
  end
end

# ── the HTTP half ─────────────────────────────────────────────────────
$jar = {}
$csrf = nil

# Sign-in credentials. The defaults are what the walk's own /first_run
# branch creates on a fresh database; a run against a SEEDED tree — the
# Rails oracle, or an emit staged with the oracle's rows — passes the
# seed's `user1@example.com` / `secret123456` instead.
EMAIL    = ENV["CAMPFIRE_EMAIL"] || "walker@example.com"
PASSWORD = ENV["CAMPFIRE_PASSWORD"] || "secret123"

def req(verb, path, form = nil, accept: "text/html")
  uri = URI("#{BASE}#{path}")
  r = verb == "GET" ? Net::HTTP::Get.new(uri) : Net::HTTP::Post.new(uri)
  r["Accept"] = accept
  r["Cookie"] = $jar.map { |k, v| "#{k}=#{v}" }.join("; ") unless $jar.empty?
  # Real Rails enforces CSRF on every POST; the emitted lanes accept the
  # header and ignore it. The token is whatever the most recent page's
  # `csrf-token` meta carried — which is how campfire's own JavaScript
  # authenticates its fetches.
  r["X-CSRF-Token"] = $csrf if verb == "POST" && $csrf
  r.set_form_data(form) if form
  res = Net::HTTP.start(uri.host, uri.port) { |h| h.request(r) }
  Array(res.get_fields("set-cookie")).each do |c|
    k, v = c.split(";", 2)[0].split("=", 2)
    $jar[k] = v
  end
  if verb == "GET" && (token = res.body.to_s[/<meta name="csrf-token" content="([^"]+)"/, 1])
    $csrf = token
  end
  res
end

# ── the socket half ───────────────────────────────────────────────────
#
# An Action Cable client in thirty lines: websocket-driver in CLIENT mode
# over a plain TCPSocket, pumped by hand. No consumer, no Stimulus, no
# browser — the frames are the contract, and asserting on them directly
# is what makes a failure say which frame was wrong rather than which
# pixel was missing.
class Client
  attr_reader :messages, :url

  def initialize(url, cookie)
    @url = url            # set BEFORE the driver: it reads `url` on build
    @socket = TCPSocket.new(URL.host, URL.port)
    @messages = []
    @driver = WebSocket::Driver.client(self, protocols: ["actioncable-v1-json"])
    @driver.set_header("Cookie", cookie)
    # Same-origin, spelled out: Action Cable's request-forgery check
    # rejects a handshake whose Origin doesn't match the host, and a
    # bare websocket-driver client sends none. The emitted lanes don't
    # check; real Rails does.
    @driver.set_header("Origin", "#{URL.scheme}://#{URL.host}:#{URL.port}")
    @driver.on(:message) { |e| @messages << (JSON.parse(e.data) rescue {}) }
    @driver.start
  end

  def write(data) = @socket.write(data)
  def send_json(hash) = @driver.text(JSON.generate(hash))

  def subscribe(identifier_json)
    send_json({ "command" => "subscribe", "identifier" => identifier_json })
  end

  # Pump until the block is satisfied or the deadline passes. Returns the
  # block's last answer, so a caller can assert on the timeout too.
  def until(seconds = 5)
    deadline = Time.now + seconds
    loop do
      got = yield
      return got if got
      return got if Time.now >= deadline
      next unless IO.select([@socket], nil, nil, 0.05)
      begin
        @driver.parse(@socket.read_nonblock(4096))
      rescue IO::WaitReadable
        next
      rescue EOFError, IOError, SystemCallError
        return yield
      end
    end
  end

  def typed(type) = @messages.any? { |m| m["type"] == type }

  # A DATA frame, which is the one WITHOUT a `type`. The server-wide
  # heartbeat is `{"type":"ping","message":<unix-ts>}` — it has a
  # `message` key too, so filtering on that alone counts pings as
  # deliveries and every assertion below would pass on a silent stream.
  def payloads = @messages.select { |m| m["type"].nil? && m.key?("message") }

  def close = (@socket.close rescue nil)
end

# ── sign in ───────────────────────────────────────────────────────────
puts "\n\e[1;34m==>\e[0m sign in"
if req("GET", "/first_run").code == "200"
  req("POST", "/first_run", {
    "user[name]" => "Walker",
    "user[email_address]" => EMAIL,
    "user[password]" => PASSWORD,
  })
else
  # A seeded tree: through the real form, scraping its CSRF token on
  # the way past — real Rails 422s a naked /session POST.
  req("GET", "/session/new")
  req("POST", "/session", {
    "email_address" => EMAIL, "password" => PASSWORD,
  })
end
cookie = $jar.map { |k, v| "#{k}=#{v}" }.join("; ")
check("the account has a session cookie", $jar.key?("session_token"), true)

# ── the page names the channel and the stream ─────────────────────────
puts "\n\e[1;34m==>\e[0m read the room"
room = req("GET", "/rooms/1")
dump("room.html", room.body)
check("GET /rooms/1", room.code, "200")
tag = room.body.to_s[/<turbo-cable-stream-source[^>]*>/]
check("the room page carries a stream source", !tag.nil?, true)
abort "no <turbo-cable-stream-source> on the room page" if tag.nil?

channel = tag[/channel="([^"]+)"/, 1]
signed  = tag[/signed-stream-name="([^"]+)"/, 1]
# The app routed the subscription AWAY from the stock channel on purpose.
check("the page names the app's own channel", channel, "RoomMessagesChannel")

# ── two connections ───────────────────────────────────────────────────
puts "\n\e[1;34m==>\e[0m two /cable connections"
ws = "#{URL.scheme == "https" ? "wss" : "ws"}://#{URL.host}:#{URL.port}/cable"
a = Client.new(ws, cookie)
b = Client.new(ws, cookie)
at_exit { a.close; b.close }

# A welcome means the handshake ran the app's `ApplicationCable::
# Connection#connect` and it did not refuse — item 3, over a real socket.
check("both connections are welcomed",
      [a.until { a.typed("welcome") }, b.until { b.typed("welcome") }], [true, true])

identifier = JSON.generate({ "channel" => channel, "signed_stream_name" => signed })
[a, b].each { |c| c.subscribe(identifier) }
check("both subscriptions are confirmed",
      [a.until { a.typed("confirm_subscription") },
       b.until { b.typed("confirm_subscription") }], [true, true])

# ── the guard, end to end ─────────────────────────────────────────────
#
# Same signed stream name, stock channel. campfire prepends
# `RoomStreamsAreAuthorized` onto `Turbo::StreamsChannel` precisely so
# this is refused — "the stock channel as a way around it: same signed
# stream name, no membership check".
puts "\n\e[1;34m==>\e[0m the guard the app installed"
stock = JSON.generate({ "channel" => "Turbo::StreamsChannel", "signed_stream_name" => signed })
a.subscribe(stock)
check("the stock channel refuses the room's :messages stream",
      a.until { a.typed("reject_subscription") }, true)

# ── the milestone ─────────────────────────────────────────────────────
puts "\n\e[1;34m==>\e[0m post a message, and watch both sockets"
before = [a.payloads.size, b.payloads.size]
BODY = "hello from the cable walk"
res = req("POST", "/rooms/1/messages",
          { "message[body]" => BODY, "message[client_message_id]" => "cable-walk-1" },
          accept: "text/vnd.turbo-stream.html, text/html")
# A MILESTONE probe. It was a ledger probe for one commit, while
# `room.memberships.pluck(:user_id)` walked off the end of a hydrated
# Array and 500'd the request AFTER its broadcast had already gone out —
# fan-out that works attached to a request that does not.
check("POST /rooms/1/messages", res.code, "200")

def received?(client, before)
  client.until(6) do
    client.payloads.drop(before).any? { |m| m["message"].to_s.include?("turbo-stream") }
  end
end
check("connection A receives the broadcast", received?(a, before[0]), true)
check("connection B receives the broadcast", received?(b, before[1]), true)

frame = a.payloads.drop(before[0]).find { |m| m["message"].to_s.include?("turbo-stream") }
dump("frame_text.html", frame ? frame["message"] : "")
if frame
  html = frame["message"].to_s
  # The frame must APPEND, and it must target a list the room page
  # actually renders — scraped from the page rather than spelled here,
  # because the name is dom_id's and the lanes disagree about it today:
  # room 1 is an STI `Rooms::Open`, Rails' dom_id says
  # `messages_rooms_open_1`, the emit's dom_prefix says
  # `messages_room_1`. Each lane is SELF-consistent (its frame targets
  # its own page's element), which is what this probe asserts; the
  # cross-lane naming divergence is scripts/campfire-compare's to
  # report.
  target = html[/<turbo-stream action="append" target="([^"]+)">/, 1].to_s
  check("it is an append to a list the room page renders",
        target != "" && room.body.to_s.include?("id=\"#{target}\""), true)
  # A MILESTONE probe, and it earned the promotion. It was a ledger probe
  # for one commit, when an inherited class-side `new` built the abstract
  # filter and the app's own `rescue Exception` turned the resulting
  # `NotImplementedError` into `""` — every message body empty behind a
  # 200 and a green suite. The frame carrying the text is the difference
  # between fan-out that works and fan-out that delivers blank bubbles.
  check("it carries the message that was posted", html.include?(BODY), true)
  # The identifier is echoed byte for byte: the client keys its
  # subscription table on it, so a re-spelled one is a frame nobody claims.
  check("the frame echoes the subscription identifier", frame["identifier"], identifier)
end

# ── an HTML body, which is what Trix actually posts ───────────────────
#
# Every probe above posts PLAIN TEXT, and that blindness cost a day:
# the safe-list sanitizer raised on any body containing markup,
# campfire's `rescue Exception` rendered it as "", and 13/13 here
# coexisted with a chat app that could not display a composer-typed
# message (Trix wraps everything in `<div>`). Two fixes later —
# the sanitizer port, and the `h()` escape-exemption for the filter
# chain's `ActionText::Content` — this probe is what keeps both honest,
# at the wire, on whichever lane runs this script. The allow-listed
# `<strong>` must arrive AS MARKUP (not `&lt;strong&gt;`), and the
# `<script>` must arrive stripped to its text.
puts "\n\e[1;34m==>\e[0m post an HTML body, which must render as markup"
before2 = a.payloads.size
req("POST", "/rooms/1/messages",
    { "message[body]" => "<div>rich <strong>bold</strong> <script>alert(1)</script></div>",
      "message[client_message_id]" => "cable-walk-2" },
    accept: "text/vnd.turbo-stream.html, text/html")
check("connection A receives the HTML-bodied broadcast", received?(a, before2), true)
frame2 = a.payloads.drop(before2).find { |m| m["message"].to_s.include?("turbo-stream") }
dump("frame_html.html", frame2 ? frame2["message"] : "")
if frame2
  html2 = frame2["message"].to_s
  check("the markup survives sanitizing un-escaped",
        html2.include?("rich <strong>bold</strong>"), true)
  check("the script tag does not",
        html2.include?("<script>") || html2.include?("&lt;script&gt;"), false)
end

puts
unless LEDGER_FAILURES.empty?
  puts "\e[33m#{LEDGER_FAILURES.length} known gap(s) on the way past:\e[0m " \
       "#{LEDGER_FAILURES.join(", ")}"
end
if MILESTONE_FAILURES.empty?
  puts "\e[1;32mcable walk complete\e[0m — a message posted over HTTP reached two live sockets"
  exit 0
else
  puts "\e[1;31mcable walk failed\e[0m — #{MILESTONE_FAILURES.length} probe(s): " \
       "#{MILESTONE_FAILURES.join(", ")}"
  exit 1
end
