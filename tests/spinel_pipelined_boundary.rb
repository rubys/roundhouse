# Exercise the real connection loops, including bytes already read for
# a successor and a successor header that needs another recv to finish.
require_relative "tep_server_harness"

class RecordingApp
  attr_reader :paths

  alias reset_bodies reset
  def reset
    reset_bodies
    @paths = []
  end

  def dispatch(req, res)
    @paths << req.path.b
    @bodies << req.raw_body.b
    res.status = 200
    res.body = "ok"
    res.start_stream(Tep::Streamer.new) if req.path == "/stream"
    res.start_websocket("test", Tep::WebSocket::Driver.new(0)) if req.path == "/upgrade"
    res.send_file(File.join(__dir__, "missing-pipelined-file")) if req.path == "/missing-file"
    res.headers["connection"] = "keep-alive, CLOSE" if req.path == "/response-close"
  end
end

# Keep the upgrade handoff synchronous without a WebSocket transport here.
# The native probe separately checks the real handshake and close frame.
class Tep::WebSocket::Connection
  def run; end
end

class Wire
  def exhausted? = @pos == @bytes.bytesize
end

# Buffered requests must not wait for fresh socket readability.
class ReadyIO
  def wait_readable(_timeout) = Sock.wire.exhausted? ? nil : self
end

module Tep::Scheduler
  def self.io_wait(_fd, _mode, _timeout) = Sock.wire.exhausted? ? 0 : 1
end

module Sock
  class << self
    attr_accessor :closes
  end

  def self.sp_net_close(_fd)
    self.closes += 1
    0
  end
end

SECOND = "GET /next HTTP/1.1\r\nHost: localhost\r\n\r\n".b
THIRD = "POST /third HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\né".b
close_first = "POST /posts HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".b
stream_first = "GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n".b
upgrade_first = "GET /upgrade HTTP/1.1\r\nHost: localhost\r\n\r\n".b
missing_first = "GET /missing-file HTTP/1.1\r\nHost: localhost\r\n\r\n".b
response_close_first = "GET /response-close HTTP/1.1\r\nHost: localhost\r\n\r\n".b
scenarios = [
  ["two-byte body", post(2, "é".b), SECOND, ["/posts", "/next"], ["é".b, ""]],
  ["zero-length body", post(0, ""), SECOND, ["/posts", "/next"], ["", ""]],
  ["absent length", post(nil, ""), SECOND, ["/posts", "/next"], ["", ""]],
  ["binary body", post(3, "\x00\xff\x80".b), SECOND, ["/posts", "/next"], ["\x00\xff\x80".b, ""]],
  ["three requests", post(2, "é".b), SECOND + THIRD, ["/posts", "/next", "/third"], ["é".b, "", "é".b]],
  ["multibyte request target", "POST /pést HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\né".b,
   SECOND, ["/pést".b, "/next"], ["é".b, ""]],
  ["body requiring a drain", post(6000, "é".b * 3000), SECOND, ["/posts", "/next"], ["é".b * 3000, ""]],
  ["Connection close", close_first, SECOND, ["/posts"], [""]],
  ["streaming close", stream_first, SECOND, ["/stream"], [""]],
  ["missing file close", missing_first, SECOND, ["/missing-file"], [""]],
  ["response Connection close", response_close_first, SECOND, ["/response-close"], [""]],
  ["WebSocket upgrade", upgrade_first, SECOND, ["/upgrade"], [""]]
]

SERVERS.each_key do |server|
  scenarios.each do |label, first, rest, paths, bodies|
    # The prefork server has no WebSocket upgrade implementation.
    next if label == "WebSocket upgrade" && server == "blocking"
    [false, true].each do |utf8|
      [1 << 20, 5, first.bytesize + 10].each do |chunk|
        Sock.wire = Wire.new(first.b + rest.b, utf8: utf8, chunk: chunk)
        Sock.closes = 0
        APP.reset
        raised = nil
        begin
          case server
          when "threaded" then Tep::Server::Threaded.handle_connection(7)
          when "scheduled" then Tep::Server::Scheduled.handle_connection(7)
          when "blocking" then Tep::Server.new(APP).handle_connection(7)
          end
        rescue StandardError => error
          raised = error
        end
        name = "#{server}: #{label}, utf8=#{utf8}, recv chunk=#{chunk}"
        detail = "paths=#{APP.paths.inspect}, sizes=#{APP.bodies.map(&:bytesize)}, raised=#{raised.inspect}"
        check("#{name}: each body has exactly its own bytes", raised.nil? && APP.bodies == bodies.map(&:b), detail)
        check("#{name}: successor paths are retained in order", raised.nil? && APP.paths == paths.map(&:b), detail)
        statuses = Sock.wire.out.scan(/HTTP\/1\.\d (\d{3})/).flatten.map(&:to_i)
        expected_statuses = if label == "WebSocket upgrade"
          server == "threaded" ? [101] : [501]
        elsif label == "missing file close"
          [404]
        else
          [200] * paths.length
        end
        check("#{name}: each request receives its response", raised.nil? && statuses == expected_statuses, statuses.inspect)
        check("#{name}: the connection closes once", raised.nil? && Sock.closes == 1, Sock.closes.inspect)
      end
    end
  end
end

# A short first recv creates the same header offset without pipelining.
class FragmentedHeaderWire < Wire
  def recv(n)
    super(@recvs == 0 ? [n, 4034].min : n)
  end
end

prefix = "POST /first HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\nab".b
SERVERS.each_key do |server|
  [65535, 65536, 66000, 69000].each do |size|
    head = "GET /large-header HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nX-Pad: ".b
    header = head + "x" * (size - head.bytesize - 4) + "\r\n\r\n"
    [false, true].each do |queued|
      [false, true].each do |utf8|
        wire_class = queued ? Wire : FragmentedHeaderWire
        Sock.wire = wire_class.new((queued ? prefix : "") + header + SECOND, utf8: utf8, chunk: 4096)
        Sock.closes = 0
        APP.reset
        case server
        when "threaded" then Tep::Server::Threaded.handle_connection(7)
        when "scheduled" then Tep::Server::Scheduled.handle_connection(7)
        when "blocking" then Tep::Server.new(APP).handle_connection(7)
        end
        paths = queued ? ["/first"] : []
        paths << "/large-header" if size <= 65535
        statuses = Sock.wire.out.scan(/HTTP\/1\.\d (\d{3})/).flatten.map(&:to_i)
        name = "#{server}: #{size}-byte header, queued=#{queued}, utf8=#{utf8}"
        check("#{name}: only headers within the limit dispatch", APP.paths == paths, APP.paths.inspect)
        check("#{name}: only accepted requests get responses", statuses == [200] * paths.length, statuses.inspect)
        check("#{name}: the connection closes once", Sock.closes == 1)
      end
    end
  end
end

# Bounds are measured at the connection-owned buffer, not on TCP input size.
if Tep.const_defined?(:InputBuffer)
  class Tep::InputBuffer
    attr_reader :high_water
    alias keep_without_measurement keep_pending_input
    def keep_pending_input(bytes)
      @high_water = [@high_water || 0, bytes.bytesize].max
      keep_without_measurement(bytes)
    end
  end
end

SERVERS.each_key do |server|
  long_stream = SECOND * 1000
  Sock.wire = Wire.new(long_stream)
  APP.reset
  Sock.closes = 0
  # Explicit handle_one calls keep the buffer available for inspection.
  input = Tep.const_defined?(:InputBuffer) ? Tep::InputBuffer.new : nil
  1000.times do
    if input
      case server
      when "threaded" then Tep::Server::Threaded.handle_one(7, ReadyIO.new, input)
      when "scheduled" then Tep::Server::Scheduled.handle_one(7, input)
      when "blocking" then Tep::Server.new(APP).handle_one(7, input)
      end
    else
      SERVERS.fetch(server).call(7)
    end
  end
  check("#{server}: a long pipeline dispatches every request", APP.paths == ["/next"] * 1000)
  check("#{server}: pending input stays below one recv", input && input.high_water < 4096)

  oversized = "POST /big HTTP/1.1\r\nHost: localhost\r\n" \
              "Content-Length: #{Tep.max_body_bytes + 1}\r\n\r\n"
  Sock.wire = Wire.new(SECOND + oversized + "x" * 8192)
  APP.reset
  case server
  when "threaded" then Tep::Server::Threaded.handle_connection(7)
  when "scheduled" then Tep::Server::Scheduled.handle_connection(7)
  when "blocking" then Tep::Server.new(APP).handle_connection(7)
  end
  check("#{server}: a buffered successor still obeys the body cap",
        APP.paths == ["/next"] && Sock.wire.out.scan(/HTTP\/1\.\d (\d{3})/).flatten == ["200", "413"])
  check("#{server}: an oversized successor never drains", Sock.wire.recvs == 1)
end

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
