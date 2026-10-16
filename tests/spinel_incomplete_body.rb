# EOF and body-read timeouts must end the connection before dispatch.
require_relative "tep_server_harness"

module Sock
  class << self
    attr_accessor :body_timeout
  end
end

class Wire
  def exhausted? = @pos == @bytes.bytesize
end

class ReadyIO
  def wait_readable(_timeout)
    Sock.body_timeout && Sock.wire.exhausted? ? nil : self
  end
end

module Tep::Scheduler
  def self.io_wait(_fd, _mode, _timeout)
    Sock.body_timeout && Sock.wire.exhausted? ? 0 : 1
  end
end

SERVERS.each_key do |server|
  [false, true].each do |timeout|
    Sock.body_timeout = timeout
    [[10000, "abc", 4096], [10, "", 4096],
     [6000, "x" * 5000, 1000], [3, "é".b, 1]].each do |length, body, chunk|
      result = serve(server, post(length, body), chunk: chunk, utf8: true)
      status, _recvs, bodies, raised, keep_alive = result
      check(
        "#{server}: #{timeout ? 'timeout' : 'EOF'} after #{body.bytesize}/#{length} bytes never dispatches",
        raised.nil? && status == 0 && Sock.wire.out.empty? && bodies.empty?,
        describe(*result)
      )
      check(
        "#{server}: an incomplete body ends the keep-alive loop",
        raised.nil? && keep_alive == false,
        describe(*result)
      )
    end
    [[2, "é".b], [0, ""], [5000, "x" * 5000]].each do |length, body|
      result = serve(server, post(length, body), chunk: 1000, utf8: true)
      status, _recvs, bodies, raised, keep_alive = result
      check(
        "#{server}: a complete #{length}-byte body still serves before #{timeout ? 'timeout' : 'EOF'}",
        raised.nil? && status == 200 && keep_alive && bodies.map(&:b) == [body.b],
        describe(*result)
      )
    end
  end
end

Sock.body_timeout = false
puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
