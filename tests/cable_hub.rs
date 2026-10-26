//! Cable::Hub (runtime/spinel/scaffold/ruby_overlay/runtime/cable_hub.rb):
//! a broadcast in one forked worker reaches the sockets of every other
//! worker that subscribed to its stream, once, with String and Hash
//! messages intact and one publisher's messages in order.

use std::process::Command;

const HUB_ROUND_TRIP: &str = r#"
require "tmpdir"
require "timeout"
require File.expand_path("runtime/spinel/scaffold/ruby_overlay/runtime/cable_hub", Dir.pwd)

# A worker's local Registry, recording what reaches its sockets.
class Local
  attr_reader :got
  def initialize = @got = Queue.new
  def broadcast(stream, message) = @got << [stream, message]
  def watch(_hub) = nil
end

def take(local, n)
  Array.new(n) { Timeout.timeout(5) { local.got.pop } }
end

def quiet?(*locals)
  sleep 0.2
  locals.all? { |local| local.got.empty? }
end

path = File.join(Dir.tmpdir, "cable-hub-test-#{Process.pid}.sock")
hub = fork { Cable::Hub.serve(path) }
begin
  a, b, c = Local.new, Local.new, Local.new
  # Connect before the hub listens: connect waits for it.
  wa, wb, wc = [a, b, c].map { |local| Cable::Hub.connect(path, local) }

  # b and c hold room:1; only c holds user_7_unreads. stream_added returns
  # once the hub has the route, so a publish right after it cannot miss.
  wb.stream_added("room:1")
  wc.stream_added("room:1")
  wc.stream_added("user_7_unreads")

  html = %(<turbo-stream action="append" target="messages"><template><p>hi\nthere</p></template></turbo-stream>)
  wa.broadcast("room:1", html)
  wa.broadcast("user_7_unreads", { roomId: 1, at: 2.5 })
  20.times { |i| wa.broadcast("room:1", "m#{i}") }

  # The sender delivers to its own sockets directly, once.
  raise "sender" unless take(a, 22).map(&:last) == [html, { roomId: 1, at: 2.5 }, *Array.new(20) { |i| "m#{i}" }]

  got_b = take(b, 21)
  raise "fragment: #{got_b[0].inspect}" unless got_b[0] == ["room:1", html]
  raise "order: #{got_b[1..].map(&:last).inspect}" unless got_b[1..].map(&:last) == Array.new(20) { |i| "m#{i}" }

  got_c = take(c, 22)
  raise "payload: #{got_c[1].inspect}" unless got_c[1] == ["user_7_unreads", { "roomId" => 1, "at" => 2.5 }]
  raise "routed past a subscriber, or delivered twice" unless quiet?(a, b, c)

  # Unsubscribed, b stops receiving; c still does.
  wb.stream_removed("room:1")
  sleep 0.1
  wa.broadcast("room:1", "after")
  raise "unsubscribe" unless take(a, 1) == [["room:1", "after"]] && take(c, 1) == [["room:1", "after"]] && quiet?(b)

  # A publisher that holds the stream itself is not sent its own message back.
  wc.broadcast("room:1", "from c")
  raise "echo" unless take(c, 1) == [["room:1", "from c"]] && quiet?(a, b, c)
  puts "ALL OK"
ensure
  Process.kill(:TERM, hub)
  Process.wait(hub)
  File.unlink(path) if File.exist?(path)
end
"#;

#[test]
fn hub_relays_each_broadcast_to_the_subscribed_workers_once_and_in_order() {
    let output = Command::new("ruby")
        .args(["-e", HUB_ROUND_TRIP])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "hub round trip failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("ALL OK"), "hub cases did not execute:\n{stdout}\n{stderr}");
}
