//! CRuby overlay `Rails::MemoryStore` (shard / RCU / Marshal).

use std::path::Path;
use std::process::Command;

#[test]
fn overlay_read_str_does_not_dup_a_cached_fragment() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/rails_cache"

store = Rails::MemoryStore.new
frag = "message-html" * 32
store.write_str("k", frag, 0)
hit = store.read_str("k")
raise "miss" if hit.nil?
raise "duped" unless hit.equal?(store.read_str("k"))
raise "mutated store" unless hit.frozen?
begin
  hit << "x"
  raise "frozen fragment was mutable"
rescue FrozenError
end
other = store.read("k")
raise "untyped read must still dup" if other.equal?(hit)
other << "x"
raise "store corrupted" unless store.read_str("k") == frag
# write (untyped) also freezes, so a later read_str cannot mutate
# the shared entry — CodeRabbit on #432.
store.write("k2", "plain")
hit2 = store.read_str("k2")
raise "write miss" if hit2.nil?
raise "write not frozen" unless hit2.frozen?
begin
  hit2 << "x"
  raise "write-path fragment was mutable"
rescue FrozenError
end
raise "write store corrupted" unless store.read_str("k2") == "plain"
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "overlay read_str failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn overlay_memory_store_shards_match_spinel_and_survive_races() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // r## so Ruby `"#{…}"` interpolations do not terminate the raw string.
    let script = r##"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/rails_cache"

raise "shard count" unless Rails::MemoryStore::SHARD_COUNT == 32

# Runtime formula parity with Spinel is gated in runtime_files.rs
# (canonical mix substring). This driver checks the live method.
def expected_shard_of(k)
  n = k.bytesize
  return 0 if n == 0
  ((k.getbyte(n - 1) * 31) + k.getbyte(n / 2)) % 32
end

store = Rails::MemoryStore.new
[
  "",
  "a",
  "views/rooms/show/messages/1-20260101120000",
  "views/rooms/show/messages/99-20260101125959",
  "views/coll/" + ("x" * 200) + "/memberships/7-1",
].each do |k|
  a = store.send(:shard_of, k)
  b = expected_shard_of(k)
  raise "shard_of mismatch for #{k.inspect}: #{a} vs #{b}" unless a == b
end

# Marshal payloads still round-trip (boxed Array encoding, not a String).
payload = { "id" => 7, "tags" => ["a", "b"] }
store.write("marshal-k", payload)
got = store.read("marshal-k")
raise "marshal miss" if got.nil?
raise "marshal equal?" if got.equal?(payload)
raise "marshal body #{got.inspect}" unless got == payload
raise "read_str must miss marshal" unless store.read_str("marshal-k").nil?

# TTL: plant an already-expired timestamp rather than sleeping past
# a 1s window (50 ms of slack flakes on a loaded CI host).
store.write_str("ttl-k", "old", 1)
raise "ttl prime" unless store.read_str("ttl-k") == "old"
ttl_s = store.send(:shard_of, "ttl-k")
store.instance_variable_get(:@shards)[ttl_s]["ttl-k"] =
  ["old".freeze, Process.clock_gettime(Process::CLOCK_MONOTONIC) - 1]
raise "ttl still live" unless store.read_str("ttl-k").nil?

# Expired-eviction re-check: reader observes stale, blocks on the shard
# lock, a fresher entry is installed before the lock is released — must
# return the fresh String, not nil (the bug if delete's miss path always
# returned after the re-check).
k = "race-ttl"
s = store.send(:shard_of, k)
past = Process.clock_gettime(Process::CLOCK_MONOTONIC) - 10
shards = store.instance_variable_get(:@shards)
mutexes = store.instance_variable_get(:@mutexes)
shards[s][k] = ["stale".freeze, past]
held = Queue.new
release = Queue.new
results = Queue.new
locker = Thread.new do
  mutexes[s].synchronize do
    held << true
    release.pop
  end
end
held.pop
reader = Thread.new { results << store.read_str(k) }
deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 2
Thread.pass until reader.status == "sleep" || !reader.alive? ||
  Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
raise "reader never blocked on shard mutex (#{reader.status.inspect})" unless reader.status == "sleep"
shards[s][k] = ["fresh".freeze, nil]
release << true
locker.join
reader.join
got = results.pop
raise "recheck missed fresh: #{got.inspect}" unless got == "fresh"
raise "store lost fresh" unless store.read_str(k) == "fresh"

# Multi-thread Puma shape: many keys across shards, RMW increment, mixed
# read_str / write_str. No lost increments; every written key readable.
n_threads = 8
n_keys = 64
rounds = 40
errs = Queue.new
threads = n_threads.times.map do |t|
  Thread.new do
    begin
      rounds.times do |r|
        k = "views/rooms/show/messages/#{(t * rounds + r) % n_keys}-ts"
        store.write_str(k, "body-#{t}-#{r}", 0)
        hit = store.read_str(k)
        raise "lost write #{k}" if hit.nil? || !hit.start_with?("body-")
        store.increment_str("rate-#{t % 4}", 60)
      end
    rescue => e
      errs << "#{e.class}: #{e.message}"
    end
  end
end
threads.each(&:join)
raise errs.pop unless errs.empty?
total = 4.times.sum { |i| store.read_str("rate-#{i}").to_i }
expected = n_threads * rounds
raise "increment lost: #{total} != #{expected}" unless total == expected

# Clear wipes every shard.
store.clear
raise "clear left str" unless store.read_str("views/rooms/show/messages/0-ts").nil?
raise "clear left rate" unless store.read_str("rate-0").nil?

puts "ALL OK"
"##;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "overlay MemoryStore shard/race failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
