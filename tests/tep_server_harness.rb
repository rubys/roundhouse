# Shared by the tests/spinel_*.rb drivers that exercise the spinel lane's
# HTTP server (`runtime/spinel/tep/`) under plain CRuby: the REAL servers
# — all three `handle_one`s (threaded — the default —, fiber-scheduled,
# and blocking/prefork) — the real Parser/Request/Response, and the real
# `Sock.sphttp_*` wrappers in net.rb. Only the sp_net primitives
# underneath are replaced, by a scripted socket that records every recv
# the server asks for and how far into the stream it read.

# net.rb declares the sp_net primitives with spinel's `ffi_func`; under
# CRuby that declaration is a no-op and the scripted socket below
# supplies the primitives, so the Ruby wrappers above them run as shipped.
class Module
  def ffi_func(*); end
end

module Tep
  # The fiber scheduler, reduced to "the fd is ready": the scheduled
  # server and its body drain park on `io_wait`, and the scripted socket
  # never blocks.
  module Scheduler
    READ = 1
    WRITE = 2
    def self.io_wait(_fd, _mode, _timeout)
      1
    end
  end
end

# tep.rb's own order, less the two files stubbed here (scheduler, app)
# and the ones only its load-time type seeding needs — that seeding
# opens a stream, which is spinel's business, not this test's.
%w[
  tep_core url net streamer broadcast_subscription websocket
  request response parser server server_threaded server_scheduled
].each do |f|
  require_relative "../runtime/spinel/tep/#{f}"
end

# One connection's bytes. `recv` serves at most `chunk` bytes per call
# (what a socket would hand back in pieces), records each call, and
# answers "" at the end — EOF. `pos` is how far into the stream the
# server has read, which is how a test sees an over-read into the next
# pipelined request.
#
# `utf8: true` hands chunks back tagged UTF-8, so `String#length` counts
# characters on whatever the server accumulates from them. That is a
# STRESS model, not a claim about every recv'd String. Probed on spinel
# 775ba5f68: bytes from `sp_net_recv_some(:binstr)` keep `length ==
# bytesize` through `+`, character slicing and `byteslice`, as CRuby's
# do, but appending them with `<<` onto `+""` yields a String whose
# `length` counts characters (the threaded and scheduled header readers
# build their blob that way, which is what 5cb6d051's `raw_body.length`
# stall came from). A wire length compared against `length` is right
# only while every String on the path happens to stay binary, and this
# mode makes any such comparison visible.
class Wire
  attr_reader :recvs, :out, :pos

  def initialize(bytes, utf8: false, chunk: 1 << 20)
    @bytes = bytes.b
    @pos = 0
    @recvs = 0
    @out = +""
    @utf8 = utf8
    @chunk = chunk
  end

  def recv(n)
    @recvs += 1
    take = [n, @chunk, @bytes.bytesize - @pos].min
    take = 0 if take < 0
    chunk = @bytes.byteslice(@pos, take)
    @pos += take
    @utf8 ? chunk.force_encoding(Encoding::UTF_8) : chunk
  end

  def write(s)
    @out << s.b
    s.bytesize
  end
end

module Sock
  class << self
    attr_accessor :wire
  end

  def self.sp_net_recv_some(_fd, n) = wire.recv(n)
  def self.sp_net_write_str(_fd, s) = wire.write(s)
  def self.sp_net_write_bytes(_fd, s, n) = wire.write(s.byteslice(0, n))
  def self.sp_net_close(_fd) = 0
end

# What the threaded server waits on between recvs.
class ReadyIO
  def wait_readable(_t) = self
end

class RecordingApp
  attr_reader :bodies

  def initialize
    @bodies = []
  end

  def reset
    @bodies = []
  end

  def dispatch(req, res)
    @bodies << req.raw_body.dup
    res.status = 200
    res.body = "ok"
  end
end

APP = RecordingApp.new
Tep.send(:remove_const, :APP) if Tep.const_defined?(:APP, false)
Tep.const_set(:APP, APP)

SERVERS = {
  "threaded" => ->(fd) { Tep::Server::Threaded.handle_one(fd, ReadyIO.new) },
  "scheduled" => ->(fd) { Tep::Server::Scheduled.handle_one(fd) },
  "blocking" => ->(fd) { Tep::Server.new(APP).handle_one(fd) },
}.freeze

CHECKS = []

def check(name, ok, detail = nil)
  CHECKS << ok
  puts "#{ok ? "ok" : "FAIL"} #{name}#{ok || detail.nil? ? "" : " — #{detail}"}"
end

def post(content_length, body)
  head = +"POST /posts HTTP/1.1\r\nHost: localhost\r\n" \
          "Content-Type: application/x-www-form-urlencoded\r\n"
  head << "Content-Length: #{content_length}\r\n" unless content_length.nil?
  head << "\r\n"
  head + body
end

# One request through one server: what it wrote back, how many recvs it
# made, and the bodies the app was dispatched with. A raise is a result
# too — on the shipped server it escapes `handle_one`.
def serve(server, bytes, **wire)
  Sock.wire = Wire.new(bytes, **wire)
  APP.reset
  begin
    SERVERS.fetch(server).call(7)
    raised = nil
  rescue StandardError => e
    raised = e
  end
  status = Sock.wire.out[/\AHTTP\/1\.\d (\d{3})/, 1].to_i
  [status, Sock.wire.recvs, APP.bodies, raised]
end

def describe(status, recvs, bodies, raised)
  return "raised #{raised.class}: #{raised.message}" if raised

  sizes = bodies.map(&:bytesize)
  "answered #{status}, #{recvs} recv(s), app dispatched with body sizes #{sizes}"
end
