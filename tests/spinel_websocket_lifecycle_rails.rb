# Optional Rails 8.1.4 oracle. Real connection/subscription/client-socket
# lifecycle, synchronous worker dispatch and an in-memory wire capture.
require "json"
begin
  gem "actioncable", ENV.fetch("RH_RAILS_ORACLE", "8.1.4")
  require "action_cable"
  require "action_cable/connection/subscriptions"
  require "action_cable/connection/client_socket"
rescue Gem::LoadError, LoadError
  puts "SKIP Rails 8.1.4 Action Cable unavailable"
  exit 0
end
require "logger"

class OraclePool
  def async_invoke(receiver, method, *args) = receiver.send(method, *args)
end

class OracleServer
  attr_reader :removed
  def initialize = @removed = 0
  def remove_connection(connection) = @removed += 1
end

class OracleConnection < ActionCable::Connection::Base
  attr_reader :wire, :socket, :disconnects, :unsubscribes
  def initialize
    @coder = ActiveSupport::JSON
    @server = OracleServer.new
    @worker_pool = OraclePool.new
    @logger = Logger.new(File::NULL)
    @wire = []
    @disconnects = @unsubscribes = 0
    @env = { "HTTP_HOST" => "example.test", "REQUEST_URI" => "/synthetic",
      "REQUEST_METHOD" => "GET", "HTTP_UPGRADE" => "websocket", "HTTP_CONNECTION" => "Upgrade",
      "HTTP_SEC_WEBSOCKET_VERSION" => "13", "HTTP_SEC_WEBSOCKET_KEY" => "dGhlIHNhbXBsZSBub25jZQ==",
      "stream.send" => ->(bytes) { @wire << bytes; bytes.bytesize } }
    @socket = ActionCable::Connection::ClientSocket.new(@env, self, Object.new, [])
    @websocket = @socket
    @subscriptions = ActionCable::Connection::Subscriptions.new(self)
    @_internal_subscriptions = nil
  end
  def on_open; end
  def disconnect = @disconnects += 1
  def note_unsubscribe = @unsubscribes += 1
  private
  def finished_request_message = "synthetic finished"
end

class OracleChannel < ActionCable::Channel::Base
  def unsubscribed = connection.note_unsubscribe
end

connection = OracleConnection.new
channel = OracleChannel.new(connection, "synthetic")
connection.subscriptions.instance_variable_get(:@subscriptions)[channel.identifier] = channel
connection.socket.start_driver
connection.wire.clear
connection.close(reason: "unauthorized", reconnect: false)
before_reply = [connection.disconnects, connection.unsubscribes]
wire_after_close = connection.wire.length
refusals = [connection.transmit(type: "late"), connection.transmit(type: "late")]
mask = [1, 2, 3, 4]
payload = [1000].pack("n")
reply = [0x88, 0x80 | payload.bytesize, *mask,
  *payload.bytes.each_with_index.map { |byte, i| byte ^ mask[i % 4] }].pack("C*")
connection.socket.parse(reply)
connection.socket.client_gone # The later EOF must not dispatch a second close.
after_reply = [connection.disconnects, connection.unsubscribes]
failures = []
failures << "late_frames_refused" unless refusals == [false, false]
failures << "no_later_wire_frames" unless connection.wire.length == wire_after_close
failures << "disconnect_and_unsubscribe_once" unless after_reply == [1, 1]
failures << "subscription_removed" unless connection.subscriptions.identifiers.empty?
puts JSON.generate(rails: ActionCable::VERSION::STRING, before_peer_reply: before_reply,
  after_peer_reply_and_eof: after_reply, refused_late_frames: refusals,
  failed_count: failures.length, failed_names: failures)
puts "PASS Rails close lifecycle" if failures.empty?
exit(failures.empty? ? 0 : 1)
