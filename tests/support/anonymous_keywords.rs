//! Shared public Ruby sample for anonymous keyword-rest forwarding tests.

pub const SOURCE: &str = r#"class KeywordForwarder
  def forward(**)
    receive(**)
  end

  def defaults(**)
    receive_defaults(**)
  end

  def defaults_with_required(**)
    target_with_default(**)
  end

  def with_local(**)
    options = {required: 99, enabled: true, token: :local}
    receive(**)
  end

  def fetch(path, **)
    request(kind: :get, path: path, **)
  end

  def fetch_effectful(tick, **)
    request(kind: tick.next_value, path: tick.next_value, **)
  end

  def fetch_duplicate_effects(tick, **)
    request(kind: tick.next_value, kind: tick.next_value, path: tick.next_value, **)
  end

  def receive(required:, enabled: true, token: :missing)
    [:base, required, enabled, token]
  end

  # The anonymous rest keeps these optional keywords as keywords during
  # ingest; optional-only signatures are still a flattened ABI boundary.
  def receive_defaults(enabled: true, token: :missing, **)
    [enabled, token]
  end

  def request(kind:, path:, token: :missing, **)
    [kind, path, token]
  end

  def target_with_default(value = 7, required:, **)
    [value, required]
  end
end

class OverriddenKeywordForwarder < KeywordForwarder
  def receive(required:, enabled: true, token: :missing)
    [:override, required, enabled, token]
  end

  def request(kind:, path:, token: :missing, **)
    [:override, kind, path, token]
  end
end

class KeywordTick
  attr_reader :count

  def initialize
    @count = 0
  end

  def next_value
    @count += 1
  end
end
"#;

pub const ASSERTIONS: &str = r#"
forwarder = KeywordForwarder.new
raise "empty packet changed defaults" unless forwarder.defaults == [true, :missing]
raise "anonymous keywords supplied a positional argument" unless forwarder.defaults_with_required(required: :ready) == [7, :ready]
raise "false or nil keywords were lost" unless forwarder.forward(required: false, enabled: false, token: nil) == [:base, false, false, nil]
raise "local variable replaced the forwarded packet" unless forwarder.with_local(required: 7, enabled: false, token: nil) == [:base, 7, false, nil]
raise "explicit keyword pairs were not forwarded" unless forwarder.fetch("/base") == [:get, "/base", :missing]
raise "forwarded keywords did not override earlier pairs" unless forwarder.fetch("/base", kind: false, path: "/packet", token: nil) == [false, "/packet", nil]

tick = KeywordTick.new
result = forwarder.forward(required: tick.next_value, enabled: tick.next_value, token: tick.next_value)
raise "effectful keyword values changed order or count" unless result == [:base, 1, 2, 3] && tick.count == 3

tick = KeywordTick.new
result = forwarder.fetch_effectful(tick, path: "/overridden")
raise "mixed keyword values changed order, count, or override semantics" unless result == [1, "/overridden", :missing] && tick.count == 2

tick = KeywordTick.new
result = forwarder.fetch_duplicate_effects(tick)
raise "duplicate keyword values changed order, last-wins, or count" unless result == [2, 3, :missing] && tick.count == 3

overridden = OverriddenKeywordForwarder.new
raise "keyword forwarding skipped the override" unless overridden.forward(required: 11, enabled: false, token: nil) == [:override, 11, false, nil]
raise "mixed keyword forwarding skipped the override" unless overridden.fetch("/base", path: "/override") == [:override, :get, "/override", :missing]

puts "anonymous keyword forwarding contract passed"
"#;
