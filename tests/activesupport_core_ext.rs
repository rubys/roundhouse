//! Small ActiveSupport surfaces that ordinary apps call. The return type
//! stays the existing duration or hash type. This does not add Date.

use roundhouse::analyze::Analyzer;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::ty::Ty;
use roundhouse::App;
use std::path::Path;
use std::process::Command;

#[test]
fn enumerable_many_and_destructive_squish_match_rails_814() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require "active_support"
require "active_support/core_ext/enumerable"
require "active_support/core_ext/string/filters"
require "active_support/core_ext/string/output_safety"
require "active_support/core_ext/array/conversions"
load ARGV.fetch(0)

def assert(value, message)
  raise message unless value
end

[
  [[1, 2, 3], ->(n) { n > 9 }, false],
  [[1, 2, 3], ->(n) { n == 2 }, false],
].each do |list, predicate, expected|
  runtime_result = ActiveSupport.many?(list, &predicate)
  rails_result = list.many?(&predicate)
  assert(runtime_result == expected && runtime_result == rails_result, "many? block zero/one-match result must match Rails 8.1.4")
end

seen = []
runtime_many = ActiveSupport.many?([1, 2, 3, 4]) { |n| seen << n; n > 1 }
assert(runtime_many && seen == [1, 2, 3], "many? block must count matches and stop on the second")
assert(ActiveSupport.many?([:a, :b]), "many? without a block")
assert(!ActiveSupport.many?([:a]), "single element many?")

rails_seen = []
rails_many = [1, 2, 3, 4].many? { |n| rails_seen << n; n > 1 }
assert(runtime_many == rails_many && seen == rails_seen, "many? must match Rails 8.1.4")

[
  ["  foo\tbar \n baz  ", "foo bar baz"],
  ["already squished", "already squished"],
  [" \t\n ", ""],
  ["\u00a0wide\u2003spacing\u2028", "wide spacing"],
  ["foo\0", "foo"],
  ["foo \0", "foo"],
].each do |input, expected|
  runtime_string = input.dup
  runtime_result = ActiveSupport.squish!(runtime_string)
  rails_string = input.dup
  rails_result = rails_string.squish!
  assert(runtime_string == expected && runtime_string == rails_string, "squish! contents")
  assert(runtime_result.equal?(runtime_string), "squish! runtime returns receiver")
  assert(rails_result.equal?(rails_string), "Rails squish! returns receiver")
end

runtime_safe = "  <b>safe</b>  ".html_safe
runtime_safe_result = ActiveSupport.squish!(runtime_safe)
rails_safe = "  <b>safe</b>  ".html_safe
rails_safe_result = rails_safe.squish!
assert(runtime_safe_result.equal?(runtime_safe) && rails_safe_result.equal?(rails_safe), "SafeBuffer squish! returns its receiver")
assert(runtime_safe == rails_safe && runtime_safe.html_safe? == rails_safe.html_safe?, "SafeBuffer squish! safety transition must match Rails 8.1.4")
assert(!runtime_safe.html_safe?, "mutating squish! must clear SafeBuffer's safety mark")

runtime_safe_noop = "already squished".html_safe
runtime_safe_noop.squish!
rails_safe_noop = "already squished".html_safe
rails_safe_noop.squish!
assert(runtime_safe_noop.html_safe? == rails_safe_noop.html_safe?, "no-op SafeBuffer squish! safety must match Rails 8.1.4")
assert(!runtime_safe_noop.html_safe?, "no-op squish! must clear the SafeBuffer safety mark")

load ARGV.fetch(1)
overlay_string = "\u00a0overlay\u2003value\u2028"
overlay_result = overlay_string.squish!
assert(overlay_string == "overlay value" && overlay_result.equal?(overlay_string), "overlay String#squish! contract")
overlay_safe = SafeString.new("  <b>safe</b>  ")
overlay_safe.squish!
assert(!overlay_safe.html_safe?, "mutating overlay squish! must clear SafeString's safety mark")
overlay_safe_noop = SafeString.new("already squished")
overlay_safe_noop.squish!
assert(!overlay_safe_noop.html_safe?, "no-op overlay squish! must clear SafeString's safety mark")
puts "ActiveSupport 8.1.4 core extension contracts passed"
"#;
    let output = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .arg(root.join("runtime/ruby/active_support_ext.rb"))
        .arg(root.join("runtime/spinel/scaffold/ruby_overlay/runtime/active_support_core_ext.rb"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("contracts passed"),
        "Rails 8.1.4 contract smoke failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}

fn body(source: &str) -> roundhouse::Expr {
    let classes = ingest_library_classes(source.as_bytes(), "ext.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes = classes;
    Analyzer::new(&app).analyze(&mut app);
    app.library_classes[0].methods[0].body.clone()
}

#[test]
fn index_with_returns_a_hash_and_durations_stay_typed() {
    let typed = body("class Probe\n  def values(items)\n    items.index_with { |item| item }\n    2.days\n    3.hours\n    4.minutes\n    1.to_d\n  end\nend\n");
    let debug = format!("{typed:?}");
    assert!(debug.contains("index_with"), "{debug}");
    assert!(matches!(typed.ty, Some(Ty::Class { .. }) | Some(Ty::Untyped) | Some(Ty::Int)), "{:?}", typed.ty);
    fn find<'a>(expr: &'a roundhouse::Expr, name: &str) -> Option<&'a roundhouse::Expr> {
        if let ExprNode::Send { method, .. } = &*expr.node {
            if method.as_str() == name {
                return Some(expr);
            }
        }
        let mut found = None;
        expr.node.for_each_child(&mut |child| {
            if found.is_none() {
                found = find(child, name);
            }
        });
        found
    }
    let indexed = body("class Probe\n  def values\n    [\"a\"].index_with { |item| item.length }\n  end\nend\n");
    let indexed_call = find(&indexed, "index_with").expect("index_with");
    assert!(matches!(indexed_call.ty, Some(Ty::Hash { .. })), "index_with: {:?}", indexed_call.ty);
    for name in ["days", "hours", "minutes"] {
        let call = find(&typed, name).expect(name);
        assert!(call.ty.is_some(), "{name} has no type");
    }
    let decimal = find(&typed, "to_d").expect("to_d");
    assert!(matches!(&decimal.ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "BigDecimal"), "{:?}", decimal.ty);
}
