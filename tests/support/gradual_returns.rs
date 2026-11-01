//! Runtime values crossing both the harvested return and parameter boundary.
pub const SOURCE: &str = r#"
class LiveRooms
  Room = Struct.new(:lock, :members)

  def self.room(id)
    state = Rails.application.config.x.live_rooms_state
    state[:lock].synchronize { state[:rooms][id] ||= Room.new(Mutex.new, {}) }
  end

  def self.join(id, socket)
    room = room(id)
    room.lock.synchronize do
      return nil if socket.closed?
      return :full if room.members.length >= 100
      room.members[socket] = true
      broadcast(room)
    end
    room
  end

  def self.broadcast(room)
    room.members.each_key { |socket| socket.deliver("presence") }
  end

  def self.open(id, socket)
    result = join(id, socket)
    return result if result == :full || result.nil?
    broadcast(result)
    result
  end
end

class RoomSocket
  attr_reader :messages

  def initialize(closed)
    @closed = closed
    @messages = []
  end

  def closed?
    @closed
  end

  def deliver(message)
    @messages << message
  end
end

module GradualConsumer
  def self.take(value)
    value
  end

  def self.value
    JSON.parse("42") || :fallback
  end

  def self.element
    [JSON.parse("42")].first || :fallback
  end

  def self.run
    take(value)
  end
end
"#;

pub const INITIALIZER: &str =
    "Rails.application.config.x.live_rooms_state = { lock: Mutex.new, rooms: {} }\n";

pub const SCRIPT: &str = r#"
first = RoomSocket.new(false)
room = LiveRooms.open(7, first)
raise "join lost Room" unless room.is_a?(LiveRooms::Room)
raise "presence lost" unless first.messages == ["presence", "presence"]
closed = RoomSocket.new(true)
raise "closed socket must return nil" unless LiveRooms.join(7, closed).nil?
raise "closed socket was added" unless room.members.length == 1
99.times { LiveRooms.join(7, RoomSocket.new(false)) }
raise "room capacity" unless room.members.length == 100
raise "full room must return :full" unless LiveRooms.join(7, RoomSocket.new(false)) == :full
raise "gradual return or parameter narrowed" unless GradualConsumer.run == 42
raise "computed return narrowed" unless GradualConsumer.take(GradualConsumer.element) == 42
puts "room, presence, closed socket, capacity, JSON consumer passed"
"#;

pub const EXPECTED: &str = "room, presence, closed socket, capacity, JSON consumer passed\n";

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write(
            "config/application.rb",
            "module GradualApp\n class Application < Rails::Application\n end\nend\n",
        )
        .write("config/initializers/live_rooms.rb", INITIALIZER)
        .write("lib/gradual_rooms.rb", SOURCE)
}
