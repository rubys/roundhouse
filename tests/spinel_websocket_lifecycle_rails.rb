# Optional Rails 8.1.4 oracle. Real connection/subscription/client-socket
# lifecycle, synchronous worker dispatch and an in-memory wire capture.
require "json"
require "logger"
begin
  gem "actioncable", ENV.fetch("RH_RAILS_ORACLE", "8.1.4")
  gem "railties", "8.1.4"
  require "rails"
  require "action_cable/engine"
  require "action_cable/connection/subscriptions"
  require "action_cable/connection/client_socket"
rescue Gem::LoadError, LoadError
  puts "SKIP Rails 8.1.4 Action Cable unavailable"
  exit 0
end
class LifecycleOracleApp < Rails::Application
  config.load_defaults ENV.fetch("RH_RAILS_DEFAULTS", "8.1").to_f
  config.root = __dir__
  config.eager_load = false
  config.secret_key_base = "synthetic-lifecycle-key" * 8
  config.logger = Logger.new(File::NULL)
end
LifecycleOracleApp.initialize!

class OraclePool
  def async_invoke(receiver, method, *args) = receiver.send(method, *args)
end

class OracleServer
  attr_reader :removed
  def initialize = @removed = 0
  def remove_connection(connection) = @removed += 1
end

class OracleConnection < ActionCable::Connection::Base
  attr_reader :wire, :socket, :disconnects, :unsubscribes, :received_close
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
  def on_close(reason, code)
    @received_close = [code, reason]
    super
  end
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
payload = [4001].pack("n") + "peer"
reply = [0x88, 0x80 | payload.bytesize, *mask,
  *payload.bytes.each_with_index.map { |byte, i| byte ^ mask[i % 4] }].pack("C*")
connection.socket.parse(reply)
connection.socket.client_gone # The later EOF must not dispatch a second close.
after_reply = [connection.disconnects, connection.unsubscribes]
failures = []
failures << "no_cleanup_before_peer_reply" unless before_reply == [0, 0]
failures << "received_close_details" unless connection.received_close == [4001, "peer"]
failures << "late_frames_refused" unless refusals == [false, false]
failures << "no_later_wire_frames" unless connection.wire.length == wire_after_close
failures << "disconnect_and_unsubscribe_once" unless after_reply == [1, 1]
failures << "subscription_removed" unless connection.subscriptions.identifiers.empty?
eof_connection = OracleConnection.new
eof_channel = OracleChannel.new(eof_connection, "synthetic-eof")
eof_connection.subscriptions.instance_variable_get(:@subscriptions)[eof_channel.identifier] = eof_channel
eof_connection.socket.start_driver
eof_connection.close(reason: "unauthorized", reconnect: false)
failures << "no_cleanup_before_eof" unless [eof_connection.disconnects, eof_connection.unsubscribes] == [0, 0]
eof_connection.socket.client_gone
eof_connection.socket.client_gone
failures << "eof_abnormal_result" unless eof_connection.received_close == [1006, ""]
failures << "eof_cleanup_once" unless [eof_connection.disconnects, eof_connection.unsubscribes] == [1, 1]
puts JSON.generate(load_defaults: ENV.fetch("RH_RAILS_DEFAULTS", "8.1"), rails: ActionCable::VERSION::STRING, before_peer_reply: before_reply,
  after_peer_reply_and_eof: after_reply, refused_late_frames: refusals,
  failed_count: failures.length, failed_names: failures)
puts "PASS Rails close lifecycle" if failures.empty?
exit(failures.empty? ? 0 : 1)
