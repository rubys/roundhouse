# Exercise the shipped threaded upgrade, driver, receive loop and broadcast
# registry over a real socket. The client deliberately does not echo CLOSE.
require_relative "../runtime/spinel/tep/tep"

class LifecycleReport
  attr_accessor :close_calls, :close_code, :close_reason, :presence_calls
  def initialize
    @close_calls = 0
    @close_code = 0
    @close_reason = +""
    @presence_calls = 0
    @open_gate = Mutex.new
    @open_ready = false
  end
  def signal_open
    @open_gate.synchronize { @open_ready = true }
    0
  end
  def await_open
    ready = false
    while !ready
      @open_gate.synchronize { ready = @open_ready }
      sleep 0.001 unless ready
    end
    0
  end
end
LIFECYCLE_REPORT = LifecycleReport.new

# Presence is a no-op in this runtime. Count the calls at that boundary.
module Tep::Presence
  def self.untrack_by_fd(fd)
    LIFECYCLE_REPORT.presence_calls += 1
    0
  end
end

class LifecycleOpen < Tep::WebSocket::Handler
  def initialize(ws, mode)
    super()
    @life_ws = ws
    @life_mode = mode
  end
  def handle_event(event)
    @life_ws.text("ready")
    LIFECYCLE_REPORT.signal_open
    raise "synthetic open callback error" if @life_mode == "open_error"
    @life_ws.close(1008, "revoked") if @life_mode == "open_close"
    0
  end
end

class LifecycleMessage < Tep::WebSocket::Handler
  def handle_event(event)
    raise "synthetic message callback error"
  end
end

def lifecycle_writes(driver, prefix)
  i = 0
  while i < 8
    driver.text(prefix + i.to_s + ":" + prefix * 65536)
    i += 1
  end
  0
end

class LifecycleClose < Tep::WebSocket::Handler
  def initialize(explode)
    super()
    @life_explode = explode
  end
  def handle_event(event)
    LIFECYCLE_REPORT.close_calls += 1
    LIFECYCLE_REPORT.close_code = event.code
    LIFECYCLE_REPORT.close_reason = event.reason
    puts "CALLBACK " + event.code.to_s + " " + event.reason
    $stdout.flush
    raise "synthetic close callback error" if @life_explode
    0
  end
end

port = ARGV[0].to_i
mode = ARGV[1]
listener = Sock.sp_net_listen_host("127.0.0.1", port, 8)
raise "listen failed" if listener < 0
puts "LISTENING"
$stdout.flush
fd = Sock.sp_net_accept(listener)
Sock.sp_net_set_nonblock(fd)
io = IO.for_fd(fd, autoclose: false)
ws = Tep::WebSocket::Driver.new(fd)
ws.set_on_open(LifecycleOpen.new(ws, mode))
ws.set_on_message(LifecycleMessage.new)
ws.set_on_close(LifecycleClose.new(mode == "close_error"))
Tep::Broadcast.subscribe_ws("synthetic", ws)
req = Tep::Request.new
req.verb = "GET"
req.path = "/synthetic"
res = Tep::Response.new
res.start_websocket("synthetic", ws)
sender = Thread.new(ws, mode) do |driver, test_mode|
  if test_mode == "concurrent"
    # Writers are admitted only after the upgrade and its open frame.
    LIFECYCLE_REPORT.await_open
    a = Thread.new(driver) { |out| lifecycle_writes(out, "A") }
    b = Thread.new(driver) { |out| lifecycle_writes(out, "B") }
    a.join
    b.join
    driver.close(1008, "revoked")
  elsif test_mode == "server" || test_mode == "race" || test_mode == "close_error"
    sleep 0.15
    driver.close(1008, "revoked")
  end
  if test_mode == "server" || test_mode == "close_error" || test_mode == "open_close"
    sleep 0.02 if test_mode == "open_close"
    puts "AFTER_CLOSE " + driver.text("late-one").to_s + " " + driver.text("late-two").to_s
    $stdout.flush
  end
end
begin
  Tep::Server::Threaded.write_response(fd, io, req, res, false)
rescue StandardError => e
  puts "ESCAPED " + e.message
ensure
  ws.retire
  io.close
  Sock.sp_net_close(fd)
end
sender.join
puts "RETIRED " + LIFECYCLE_REPORT.close_calls.to_s + " " +
  LIFECYCLE_REPORT.presence_calls.to_s + " " + Tep::Broadcast.subscriber_count.to_s
$stdout.flush
# A callback failure must leave the worker able to serve another socket.
puts "NEXT"
$stdout.flush
fd = Sock.sp_net_accept(listener)
Tep::Server::Threaded.send_simple(fd, 200, "alive")
Sock.sp_net_close(fd)
Sock.sp_net_close(listener)
