# Driver for tests/spinel_websocket_frame_cap.rs — the inbound frame-size
# cap in `runtime/spinel/tep/websocket/frame.rb`.
#
# Run under plain CRuby: the codec is pure Ruby (`getbyte`, `pack`, and
# arithmetic), unlike db.rb's sqlite3 `ffi_func`s, so no spinel build is
# needed to exercise the shipped logic.
#
# `Tep::WebSocket::DEFAULT_MAX_FRAME` and `CLOSE_MESSAGE_TOO_BIG` were
# defined, and `Driver#set_max_frame_size` stored a value, but nothing
# ever read either: `parse_from_buf` took no cap and the recv loop
# (`websocket/connection.rb`) never consulted one. So an oversized frame
# answered "need more bytes" — and `Connection#run` responds to "need" by
# appending the next recv to its accumulator and parsing again. A client
# that sent a 14-byte header advertising a 64-bit length and then
# streamed bytes grew that accumulator without bound: exactly the OOM the
# constant's comment says the cap prevents ("bounded so an oversized
# frame can be closed with 1009 rather than OOM-ing the worker").
#
# The load-bearing property is that the refusal is reachable from the
# HEADER ALONE. A cap that answered "need" until the payload arrived
# would still admit the bytes it exists to refuse.
require_relative "../runtime/spinel/tep/websocket"
# `Driver.new` builds a Handler, which builds a `Tep::Request` — needed
# only by the last two checks, which read the cap off a real driver.
require_relative "../runtime/spinel/tep/tep_core"
require_relative "../runtime/spinel/tep/request"

CHECKS = []

def check(name, ok)
  CHECKS << ok
  puts "#{ok ? "ok" : "FAIL"} #{name}"
end

MASK = [0x37, 0xfa, 0x21, 0x3d].freeze

# One masked client frame. `advertise` sets the wire length field
# independently of the bytes actually supplied, which is how a client
# claims a payload it has not sent; `form` forces a length encoding so
# the 16-bit and 64-bit paths can be reached with any size.
def client_frame(opcode: 0x02, payload: +"", advertise: nil, form: nil, fin: true)
  plen = advertise.nil? ? payload.bytesize : advertise
  form ||= if plen <= 125 then :tiny elsif plen <= 65535 then :short else :long end

  head = [(fin ? 0x80 : 0x00) | (opcode & 0x0f)]
  case form
  when :tiny
    head << (0x80 | plen)
  when :short
    head << (0x80 | 126)
    head << ((plen >> 8) & 0xff)
    head << (plen & 0xff)
  when :long
    head << (0x80 | 127)
    7.downto(0) { |i| head << ((plen >> (i * 8)) & 0xff) }
  end
  head.concat(MASK)

  masked = payload.bytes.each_with_index.map { |b, i| b ^ MASK[i & 3] }
  (head + masked).pack("C*")
end

def parse(bytes, max)
  Tep::WebSocket::Frame.parse_from_buf(bytes, 0, bytes.bytesize, max)
end

TOO_BIG = Tep::WebSocket::CLOSE_MESSAGE_TOO_BIG

# ── The header-only refusal ──────────────────────────────────────────
# No payload bytes supplied: the verdict must come off the length field.
huge = client_frame(advertise: (1 << 63) - 1)
r = parse(huge, Tep::WebSocket::DEFAULT_MAX_FRAME)
check("a 64-bit oversize is refused, not buffered", r.outcome == "close")
check("the refusal carries close code 1009", r.close_code == TOO_BIG)
check("the refusal consumes no bytes", r.consumed == 0)
check("the refusal needs no payload bytes", huge.bytesize == 14)

# A 64-bit length with its most significant bit set (RFC 6455 §5.2: it
# MUST be 0). Under CRuby it decodes to a Bignum the cap refuses anyway;
# under spinel, whose Integer is a fixed int64, the decode loop's final
# `<<` overflows — a raise inside the recv loop on current spinel, and
# on a wrapping one a NEGATIVE length that slips under the cap, skips
# the "need" check and reports a negative `consumed`. It has to be
# refused off the first length byte, before anything is accumulated,
# and as the protocol error it is.
msb = [0x82, 0x80 | 127, 0x80, 0, 0, 0, 0, 0, 0, 0x10, *MASK].pack("C*")
r = parse(msb, Tep::WebSocket::DEFAULT_MAX_FRAME)
check(
  "a 64-bit length with the high bit set is a 1002",
  r.outcome == "close" && r.close_code == Tep::WebSocket::CLOSE_PROTOCOL_ERROR
)

# Every length encoding reaches the guard, not just the 64-bit one.
short = client_frame(advertise: 65535, form: :short)
r = parse(short, 1024)
check("a 16-bit oversize is refused", r.outcome == "close" && r.close_code == TOO_BIG)

tiny = client_frame(payload: "A" * 100, form: :tiny)
r = parse(tiny, 64)
check("a 7-bit oversize is refused", r.outcome == "close" && r.close_code == TOO_BIG)

# ── The boundary ─────────────────────────────────────────────────────
at_cap = client_frame(payload: "B" * 200)
r = parse(at_cap, 200)
check("a payload exactly at the cap parses", r.outcome == "ok")
check("the at-cap payload round-trips", r.frame.payload == "B" * 200)

r = parse(at_cap, 199)
check("one byte over the cap is refused", r.outcome == "close" && r.close_code == TOO_BIG)

# ── What the cap must NOT break ──────────────────────────────────────
# A genuinely partial frame under the cap still has to ask for more, or
# the guard would close every fragmented read the recv loop depends on.
whole = client_frame(payload: "C" * 50)
partial = whole.byteslice(0, whole.bytesize - 10)
r = parse(partial, Tep::WebSocket::DEFAULT_MAX_FRAME)
check("a short read under the cap still says need", r.outcome == "need")

r = parse(whole, Tep::WebSocket::DEFAULT_MAX_FRAME)
check("a normal frame still parses", r.outcome == "ok" && r.frame.payload == "C" * 50)

# The pre-existing structural checks run before the size guard, so an
# oversized control frame is still the protocol error it was (1002), not
# reclassified as 1009.
fat_ping = client_frame(opcode: 0x09, payload: "D" * 126)
r = parse(fat_ping, Tep::WebSocket::DEFAULT_MAX_FRAME)
check(
  "an oversized control frame is still 1002",
  r.outcome == "close" && r.close_code == Tep::WebSocket::CLOSE_PROTOCOL_ERROR
)

# ── The cap is the driver's, and configurable ────────────────────────
# `set_max_frame_size` has to reach the parse the recv loop performs;
# storing a value nothing reads is the defect this file pins.
d = Tep::WebSocket::Driver.new(-1)
check("a fresh driver carries the 16 MiB default", d.max_frame_size == Tep::WebSocket::DEFAULT_MAX_FRAME)
d.set_max_frame_size(128)
r = parse(client_frame(payload: "E" * 256), d.max_frame_size)
check("the configured cap is the one enforced", r.outcome == "close" && r.close_code == TOO_BIG)

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
