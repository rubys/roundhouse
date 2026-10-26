//! The CRuby overlay's ActiveSupport::JSON fast path
//! (ruby_overlay/runtime/active_support_json_cruby.rb) writes the same
//! document as the shared encoder for every value: the C generator's
//! strict mode for JSON-native values, the shared `jsonable` walk for
//! everything strict mode refuses.

use std::path::Path;
use std::process::Command;

#[test]
fn the_overlay_encoder_writes_the_shared_encoders_document() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require "json"
require_relative "runtime/ruby/json_builder"
require_relative "runtime/ruby/active_support_ext"
J = ActiveSupport::JSON
shared = J.method(:encode)
shared_plain = J.method(:encode_without_escape)
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/active_support_json_cruby"

class SafeText < String
  def to_json(*) = "\"SAFE\""
end
class Bag < Hash; end
bag = Bag.new; bag["k"] = "v"
values = [
  nil, true, 1, 2**70, 1.5, "<b>&</b> ", :sym, [], {},
  [{ "body" => "<div>x &amp; y</div>", "id" => 7, "n" => nil }, { a: :b }, [1, [2, [3]]]],
  { 1 => "integer key" }, [Float::NAN], [Float::INFINITY], [Time.utc(2026, 10, 22, 1, 2, 3, 456789)],
  [SafeText.new("<x>")], [bag],
]
values.each do |v|
  raise "encode differs for #{v.inspect}" unless J.encode(v) == shared.call(v)
  raise "encode_without_escape differs for #{v.inspect}" unless J.encode_without_escape(v) == shared_plain.call(v)
end
raise "escaping lost" unless J.encode(["<"]) == '["<"]'.gsub("<", "\\u003c")

# What neither path models still raises, the same error.
[Object.new, [Object.new]].each do |v|
  begin
    J.encode(v)
    raise "#{v.inspect} encoded"
  rescue ArgumentError
  end
end

# The fast path is taken: a native value never reaches the walk.
walks = 0
J.singleton_class.prepend(Module.new { define_method(:jsonable) { |x| walks += 1; super(x) } })
J.encode([{ "a" => "<b>", "n" => 1 }])
raise "a native value took the walk" unless walks == 0
J.encode([Time.utc(2026)])
raise "a Time skipped the walk" unless walks > 0
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
        "overlay JSON encoder failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
