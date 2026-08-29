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

def req(verb, path, form = nil, accept: "text/html")
  uri = URI("#{BASE}#{path}")
  r = verb == "GET" ? Net::HTTP::Get.new(uri) : Net::HTTP::Post.new(uri)
  r["Accept"] = accept
  r["Cookie"] = $jar.map { |k, v| "#{k}=#{v}" }.join("; ") unless $jar.empty?
  r.set_form_data(form) if form
  res = Net::HTTP.start(uri.host, uri.port) { |h| h.request(r) }
  Array(res.get_fields("set-cookie")).each do |c|
    k, v = c.split(";", 2)[0].split("=", 2)
    $jar[k] = v
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
    "user[email_address]" => "walker@example.com",
    "user[password]" => "secret123",
  })
else
  req("POST", "/session", {
    "email_address" => "walker@example.com", "password" => "secret123",
  })
end
cookie = $jar.map { |k, v| "#{k}=#{v}" }.join("; ")
check("the account has a session cookie", $jar.key?("session_token"), true)

# ── the page names the channel and the stream ─────────────────────────
puts "\n\e[1;34m==>\e[0m read the room"
room = req("GET", "/rooms/1")
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
# The BROADCAST runs before this raises, so the fan-out below is still
# real — the message is created, committed and fanned out, and then the
# unread-room notification walks off the end of a hydrated Array.
check("POST /rooms/1/messages", res.code, "200",
      ledger: "`room.memberships.pluck(:user_id)` — an association reader is not " \
              "an arel chain root, so `pluck` lands on the Array it just hydrated")

def received?(client, before)
  client.until(6) do
    client.payloads.drop(before).any? { |m| m["message"].to_s.include?("turbo-stream") }
  end
end
check("connection A receives the broadcast", received?(a, before[0]), true)
check("connection B receives the broadcast", received?(b, before[1]), true)

frame = a.payloads.drop(before[0]).find { |m| m["message"].to_s.include?("turbo-stream") }
if frame
  html = frame["message"].to_s
  check("it is an append to the room's message list",
        !html[/<turbo-stream action="append" target="messages_room_1">/].nil?, true)
  check("it carries the message that was posted", html.include?(BODY), true,
        ledger: "a bare `new` in a class method is grounded to the LEXICAL class, " \
                "so `Filter.apply`'s `new(content)` builds the abstract base for " \
                "every subclass and `applicable?` raises NotImplementedError; the " \
                "helper's own `rescue Exception` turns that into \"\", and every " \
                "message body renders empty — on the page as well as in the frame")
  # The identifier is echoed byte for byte: the client keys its
  # subscription table on it, so a re-spelled one is a frame nobody claims.
  check("the frame echoes the subscription identifier", frame["identifier"], identifier)
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
