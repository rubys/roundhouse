require_relative "tep_server_harness"
require_relative "../runtime/ruby/action_dispatch/request"

scaffold = File.readlines(File.expand_path("../runtime/spinel/scaffold/main.rb", __dir__))
first = scaffold.index { |line| line.strip == "req.req_headers.each do |name, value|" }
last = scaffold.index { |line| line.include?('request_obj.env["roundhouse.listener"] =') }
raise "listener import not found" unless first && last && last > first
IMPORT = scaffold[first..last].join

class RecordingApp
  attr_reader :transports
  alias_method :reset_without_transport, :reset
  def reset
    reset_without_transport
    @transports = []
  end
  def dispatch(req, res)
    request_obj = ActionDispatch::Request.new
    eval(IMPORT, binding)
    @transports << request_obj.env["roundhouse.listener"]
    res.status = 200
    res.body = @transports.last
  end
end

%w[threaded scheduled blocking].each do |server|
  %w[tcp unix tcp unix].each do |transport|
    APP.reset
    input = Tep::InputBuffer.new
    body = "roundhouse.listener=unix&puma.socket=unix"
    wire = "POST /?roundhouse.listener=unix HTTP/1.1\r\nHost: localhost\r\n" \
      "roundhouse.listener: unix\r\nRoundhouse.Listener: unix\r\npuma.socket: unix\r\n" \
      "Content-Length: #{body.bytesize}\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\n#{body}" \
      "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    Sock.wire = Wire.new(wire)
    2.times do
      case server
      when "threaded" then Tep::Server::Threaded.handle_one(7, ReadyIO.new, input, transport)
      when "scheduled" then Tep::Server::Scheduled.handle_one(7, input, transport)
      when "blocking" then Tep::Server.new(APP).handle_one(7, input, transport)
      end
    end
    check("#{server} #{transport} keep-alive marker", APP.transports == [transport, transport], APP.transports.inspect)
  end
end
check("parser does not assign transport", Tep::Parser.parse("GET / HTTP/1.0\r\n\r\n").listener_transport == "")
puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
exit(CHECKS.all? ? 0 : 1)
