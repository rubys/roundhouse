//! The CRuby overlay's `action_view_helpers_cruby.rb` respells three
//! shared ViewHelpers methods (`boolean_attr?`, `needs_url_escape?`,
//! `render_attrs`) for speed. Its contract is the shared bodies' output,
//! byte for byte: this drives both over generated inputs (every boolean
//! attribute name and near misses, every escape character, nested data
//! hashes, nil and "false" values) and compares.

use std::path::Path;
use std::process::Command;

#[test]
fn cruby_view_helper_spellings_match_the_shared_bodies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require_relative "runtime/ruby/action_view/view_helpers"
VH = ActionView::ViewHelpers
shared = {}
%i[boolean_attr? needs_url_escape? render_attrs].each { |m| shared[m] = VH.method(m) }
# The shared render_attrs calls boolean_attr? by name; keep the shared one
# reachable for it while the overlay's is installed.
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/action_view_helpers_cruby"
overlay = {}
%i[boolean_attr? needs_url_escape? render_attrs].each { |m| overlay[m] = VH.method(m) }

names = VH::BOOLEAN_ATTRIBUTE_SET.keys + %w[class id href data aria hiddenx Hidden disable checkedd type value]
names.each do |n|
  a = shared[:boolean_attr?].call(n)
  b = overlay[:boolean_attr?].call(n)
  raise "boolean_attr?(#{n.inspect}) #{a} vs #{b}" unless a == b
end
raise "set lost an entry" unless VH::BOOLEAN_ATTRIBUTE_SET.size == 45

rng = Random.new(7)
alphabet = (32..126).map(&:chr) + ["\r", "\n", "\0", "é", "\t"]
2000.times do
  s = (0...rng.rand(0..12)).map { alphabet[rng.rand(alphabet.length)] }.join
  a = shared[:needs_url_escape?].call(s)
  b = overlay[:needs_url_escape?].call(s)
  raise "needs_url_escape?(#{s.inspect}) #{a} vs #{b}" unless a == b
end

values = [nil, true, false, "false", "x", "a\"b<c>&'", 0, 42, :sym, "", "é"]
keys = [:class, :id, :hidden, :disabled, :checked, :href, "title", :data_x, :open]
2000.times do
  attrs = {}
  rng.rand(0..5).times do
    k = keys[rng.rand(keys.length)]
    if rng.rand(5).zero?
      attrs[[:data, :aria][rng.rand(2)]] = (0...rng.rand(0..3)).to_h { |i| [[:foo_bar, :x, "y_z", :n][i], values[rng.rand(values.length)]] }
    else
      attrs[k] = values[rng.rand(values.length)]
    end
  end
  # Shared body, with the shared boolean_attr? (it calls by name).
  VH.define_singleton_method(:boolean_attr?, shared[:boolean_attr?])
  a = shared[:render_attrs].call(attrs)
  VH.define_singleton_method(:boolean_attr?, overlay[:boolean_attr?])
  b = overlay[:render_attrs].call(attrs)
  raise "render_attrs(#{attrs.inspect})\n  #{a.inspect}\n  #{b.inspect}" unless a == b && a.encoding == b.encoding
end
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
        "view helper spellings diverged\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
