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

def req(verb, path, form = nil, accept: "text/html", csrf: true, origin: nil, token: nil, headers: {})
  uri = URI("#{BASE}#{path}")
  r = verb == "GET" ? Net::HTTP::Get.new(uri) : Net::HTTP::Post.new(uri)
  r["Accept"] = accept
  headers.each { |k, v| r[k] = v }
  r["Cookie"] = $jar.map { |k, v| "#{k}=#{v}" }.join("; ") unless $jar.empty?
  # Every lane enforces CSRF on a POST. The token is whatever the most
  # recent page's `csrf-token` meta carried — which is how campfire's
  # own JavaScript authenticates its fetches. `csrf: false`, `origin:`
  # and `token:` (a token of the caller's choosing) are for the forgery
  # probes, which play the other site.
  r["X-CSRF-Token"] = token || $csrf if verb == "POST" && (token || $csrf) && csrf
  r["Origin"] = origin if origin
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

  def initialize(url, cookie, origin: "#{URL.scheme}://#{URL.host}:#{URL.port}")
    @url = url            # set BEFORE the driver: it reads `url` on build
    @socket = TCPSocket.new(URL.host, URL.port)
    @messages = []
    @driver = WebSocket::Driver.client(self, protocols: ["actioncable-v1-json"])
    @driver.set_header("Cookie", cookie)
    # Same-origin by default, spelled out: Action Cable's request-forgery
    # check rejects a handshake whose Origin doesn't match the host, and
    # a bare websocket-driver client sends none. `origin: nil` sends none.
    @driver.set_header("Origin", origin) if origin
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
# Both sign-in pages render campfire's `translation_button`: a <details>
# whose <summary> is the globe icon and whose popup is the language
# list. The two are sibling tag captures joined by `+`, and spinel once
# evaluated the right one first (matz/spinel#5574) — the summary held
# the whole list, an undismissable overlay on the deployed page.
def check_translation_button(body)
  summary = body.to_s[%r{<summary class="btn"[^>]*>(.*?)</summary>}m, 1].to_s
  check("the translate button's summary is the globe icon", summary.include?("globe"), true)
  check("the translate button's summary holds no language list", summary.include?("language-list"), false)
end

first_run = req("GET", "/first_run")
if first_run.code == "200"
  check_translation_button(first_run.body)
  req("POST", "/first_run", {
    "user[name]" => "Walker",
    "user[email_address]" => EMAIL,
    "user[password]" => PASSWORD,
  })
else
  # A seeded tree: through the real form, scraping its CSRF token on
  # the way past — real Rails 422s a naked /session POST.
  check_translation_button(req("GET", "/session/new").body)
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

# ── behind a TLS-terminating proxy ────────────────────────────────────
#
# A deploy behind Fly / a load balancer reaches the app over plain http
# with `X-Forwarded-Proto: https`. Rails builds every absolute URL with
# the REQUEST's scheme, so the room's refresh URL is https there; a
# literal `http://` was mixed content on the https page and the browser
# blocked the room's catch-up fetch (found on the first Fly deploy).
puts "\n\e[1;34m==>\e[0m behind a TLS-terminating proxy"
proxied = req("GET", "/rooms/1", headers: { "X-Forwarded-Proto" => "https" })
refresh = proxied.body.to_s[/data-refresh-room-url-value="([^"]+)"/, 1].to_s
check("the room's refresh URL takes the proxy's scheme", refresh[%r{\A[a-z]+://}], "https://")

# ── the PWA endpoints ─────────────────────────────────────────────────
#
# The notification bell registers `/service-worker.js` before it asks for
# permission; an empty 204 there fails the registration silently and no
# push subscription can start. The layout links the manifest as
# `webmanifest_path(format: :json)`.
puts "\n\e[1;34m==>\e[0m the PWA endpoints"
sw = req("GET", "/service-worker.js", accept: "*/*")
check("GET /service-worker.js", sw.code, "200")
check("the service worker is served as JavaScript", sw["content-type"].to_s[/\A[^;]+/], "text/javascript")
check("the service worker handles push", sw.body.to_s.include?('addEventListener("push"'), true)
manifest = req("GET", "/webmanifest.json", accept: "application/json")
check("GET /webmanifest.json", manifest.code, "200")
check("the manifest is served as JSON", manifest["content-type"].to_s[/\A[^;]+/], "application/json")
manifest_name = (JSON.parse(manifest.body.to_s)["name"] rescue nil)
check("the manifest parses and names the app", manifest_name.is_a?(String) && !manifest_name.empty?, true)

# ── a direct room in the sidebar ──────────────────────────────────────
#
# The sidebar's direct-room partial asks `members.many?` of
# `room.users.without(user).presence || [user]` — a Relation or an Array.
# The Array half had no `many?` on spinel, and every sidebar render with
# a direct room 500'd on the deployed binary. Creating one (with user 2
# on a seeded tree; alone, on a fresh one — the Array branch) and
# rendering the sidebar exercises both.
puts "\n\e[1;34m==>\e[0m a direct room in the sidebar"
direct = req("POST", "/rooms/directs", { "user_ids[]" => "2" })
check("POST /rooms/directs", direct.code, "302")
# `Rooms::Direct.find_for` searches `all` — the DIRECT rooms. Unscoped,
# the open room 1 (whose members can be exactly these users) matched,
# and starting a DM landed there instead of in a direct room.
check("starting a DM does not land in the open room",
      direct["location"].to_s.end_with?("/rooms/1"), false)
sidebar = req("GET", "/users/me/sidebar")
check("GET /users/me/sidebar with a direct room", sidebar.code, "200")
# A GROUP direct room (three others on a seeded tree) takes the partial's
# other branch, `members.first(4)` — the Relation half's `first` took no
# count, and that sidebar 500'd on both lanes. Found by the fixture seed
# scripts/campfire-http-shape runs, which has a four-person direct room.
group = req("POST", "/rooms/directs", { "user_ids[]" => %w[2 3 4] })
check("POST /rooms/directs (a group)", group.code, "302")
sidebar = req("GET", "/users/me/sidebar")
check("GET /users/me/sidebar with a group direct room", sidebar.code, "200")
# A 302 and a 200 are not the room: the spinel binary answered both
# while dropping `user_ids[]` (only the last value of an array param
# survived its parser), creating every direct room with its creator
# alone — and these probes stayed green. On a tree where the others
# exist, the group room must render the partial's many-members branch.
# (A milestone since runtime/spinel/param_builder.rb.)
if req("GET", "/users/4").code == "200"
  check("the group direct room has its members", sidebar.body.to_s.include?("avatar__group"), true)
end

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

# A handshake from another site carries the victim's cookie (a browser
# attaches it) but not the page's Origin. Action Cable refuses it with a
# 404 before `connect` runs, and it refuses a handshake with no Origin at
# all. Before the check landed, both emitted lanes welcomed both.
foreign_ws = Client.new(ws, cookie, origin: "https://attacker.example")
bare_ws = Client.new(ws, cookie, origin: nil)
at_exit { foreign_ws.close; bare_ws.close }
check("a socket from a foreign Origin is not welcomed",
      foreign_ws.until(2) { foreign_ws.typed("welcome") }, false)
check("a socket with no Origin is not welcomed",
      bare_ws.until(2) { bare_ws.typed("welcome") }, false)

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

# ── a forged request, which must be refused ───────────────────────────
#
# The other site's view of the room: the victim's cookies ride along (a
# browser attaches them), but the page's token cannot — so a POST
# without it is Rails' `InvalidAuthenticityToken`, a 422, and no message
# is created or broadcast. A token lifted some other way still fails
# from a foreign Origin. Real Rails answers both this way, which is what
# makes these probes ground truth rather than the emit agreeing with
# itself; before the forgery check landed, both emitted lanes posted the
# message.
#
# The probes send what a BROWSER sends from the other site:
# `Sec-Fetch-Site: cross-site`. Rails main checks that header before any
# token, and under `load_defaults 8.2` (`:header_only`) a POST with no
# header at all over plain http is ACCEPTED — that is a non-browser
# client, which the header cannot vouch for and CSRF does not threaten.
# Leaving the header off made the probe ask Rails a question whose
# answer depends on the strategy rather than on the forgery.
puts "\n\e[1;34m==>\e[0m a forged post, which must be refused"
before3 = a.payloads.size
forged = req("POST", "/rooms/1/messages",
             { "message[body]" => "forged", "message[client_message_id]" => "cable-walk-forged" },
             accept: "text/vnd.turbo-stream.html, text/html", csrf: false,
             headers: { "Sec-Fetch-Site" => "cross-site" })
check("a POST without the token is refused", forged.code, "422")
foreign = req("POST", "/rooms/1/messages",
              { "message[body]" => "forged", "message[client_message_id]" => "cable-walk-foreign" },
              accept: "text/vnd.turbo-stream.html, text/html", origin: "https://attacker.example")
check("the right token from a foreign Origin is refused", foreign.code, "422")
check("neither reaches a socket",
      a.until(2) { a.payloads.drop(before3).any? { |m| m["message"].to_s.include?("forged") } }, false)

# A session cookie the client wrote itself. Whoever can set a cookie for
# the domain (a sibling subdomain, a script with cookie access) could
# otherwise plant a CSRF token of their choosing and send the same one in
# the header: the check compares the two and they agree. Rails' session
# cookie is authenticated, so a plaintext one restores nothing and the
# post is refused; so must ours. The real cookie is put back afterwards,
# signed-in state and all.
#
# `Sec-Fetch-Site: none` keeps the header from vouching (a same-site
# value would pass Rails main before the token is read): under
# `:header_or_legacy_token` the token decides, under `:header_only` the
# header alone refuses. Either way the planted token must not help.
SESSION_COOKIE = "_campfire_session"
PLANTED = "planted-token-planted-token-planted-tok"
real_session = $jar[SESSION_COOKIE]
$jar[SESSION_COOKIE] = "_csrf_token=#{PLANTED}"
planted = req("POST", "/rooms/1/messages",
              { "message[body]" => "forged", "message[client_message_id]" => "cable-walk-planted" },
              accept: "text/vnd.turbo-stream.html, text/html", token: PLANTED,
              headers: { "Sec-Fetch-Site" => "none" })
real_session.nil? ? $jar.delete(SESSION_COOKIE) : $jar[SESSION_COOKIE] = real_session
check("a planted session cookie's token is refused", planted.code, "422")

# ── the flash, which only the server may write ────────────────────────
#
# A notice set on a redirect must reach the next page — through this
# runtime's signed flash cookie, or Rails' session — and a flash cookie
# the client wrote itself must not: Rails keeps the flash in the
# authenticated session and ignores a `flash_notice` cookie outright, and
# ours must reject one it did not sign. The layout renders the notice in
# its screen-reader span, which is what both checks read.
puts "\n\e[1;34m==>\e[0m a flash notice, which only the server may write"
FLASH_SPAN = %(aria-atomic="true">)
utf8 = ->(res) { res.body.to_s.dup.force_encoding("UTF-8") }
req("GET", "/account/custom_styles/edit")
styled = req("POST", "/account/custom_styles", { "_method" => "patch", "account[custom_styles]" => "" })
check("the custom styles update redirects", %w[302 303].include?(styled.code), true)
shown = req("GET", "/account/custom_styles/edit")
check("the next page shows the notice it set", utf8.(shown).include?("#{FLASH_SPAN}✓</span>"), true)
PLANTED_NOTICE = "Planted-by-the-client"
$jar["flash_notice"] = PLANTED_NOTICE
room_again = req("GET", "/rooms/1")
$jar.delete("flash_notice")
check("a flash cookie the client wrote is not shown", utf8.(room_again).include?(PLANTED_NOTICE), false)

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
