//! `Hash#to_query` and `Object#presence_in` on the public read path.
//!
//! EngineeredAt's `redirect_noncanonical_query_parameters` redirects a
//! request to `"#{request.path}?#{cleaned_params.to_query}"` with a 301,
//! so the query string is a URL search engines keep: it must be
//! byte-identical to Rails'. The expectations below were produced by
//! running activesupport 8.1.4 (`ruby -ractive_support/all`), not
//! derived from the implementation, and chosen to separate the real
//! grammar from the plausible wrong ones: keys sorted instead of the
//! encoded pairs, `%20` instead of `+`, an empty container kept, an
//! array hash sorted, nil rendered as `k=`.

use std::path::Path;
use std::process::Command;

#[test]
fn hash_to_query_matches_activesupport_8_1_4_byte_for_byte() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"

load ARGV.fetch(0)   # runtime/ruby/active_support_ext.rb

CASES = [
  [{"sort" => "top", "page" => "2"}, "page=2&sort=top"],
  [{"b" => "1", "a" => "2"}, "a=2&b=1"],
  [{"user" => {"name" => "Al", "age" => 3}, "z" => "1"}, "user%5Bage%5D=3&user%5Bname%5D=Al&z=1"],
  [{"tags" => ["rails", "a b"], "q" => "x"}, "q=x&tags%5B%5D=rails&tags%5B%5D=a+b"],
  [{"k" => nil, "a" => "1"}, "a=1&k"],
  [{"q" => "a b&c=d", "héllo" => "naïve ✓"}, "h%C3%A9llo=na%C3%AFve+%E2%9C%93&q=a+b%26c%3Dd"],
  [{"a" => true, "b" => false, "c" => 1.5, "d" => :sym}, "a=true&b=false&c=1.5&d=sym"],
  [{"e" => [], "f" => {}, "g" => "1"}, "g=1"],
  [{"a" => [{"x" => "2", "b" => "1"}, {"x" => "3"}]}, "a%5B%5D%5Bx%5D=2&a%5B%5D%5Bb%5D=1&a%5B%5D%5Bx%5D=3"],
  [{"a" => [[]]}, "a%5B%5D%5B%5D"],
  [{"a" => {"b" => {"c" => ["1", "2"]}}}, "a%5Bb%5D%5Bc%5D%5B%5D=1&a%5Bb%5D%5Bc%5D%5B%5D=2"],
  [{"z" => "1", "a%" => "+/~*"}, "a%25=%2B%2F~%2A&z=1"],
  [{"t" => "a\tb\u00a0"}, "t=a%09b%C2%A0"],
  [{}, ""],
]

CASES.each do |hash, expected|
  got = ActiveSupport.to_query(hash)
  raise "to_query(#{hash.inspect}): expected #{expected.inspect}, got #{got.inspect}" unless got == expected
end

begin
  ActiveSupport.to_query("not a hash")
  raise "a non-Hash receiver must raise, as Object#to_query without its key does"
rescue ArgumentError
end

PRESENCE = [
  ["asc", %w[asc desc], "asc"],
  ["bogus", %w[asc desc], nil],
  [nil, %w[asc desc], nil],
  ["", [""], ""],
  ["ASC", %w[asc desc], nil],
  [1, %w[1], nil],
]
PRESENCE.each do |value, list, expected|
  got = ActiveSupport.presence_in(value, list)
  raise "presence_in(#{value.inspect}, #{list.inspect}): expected #{expected.inspect}, got #{got.inspect}" unless got == expected
end
puts "to_query and presence_in contracts passed"
"#;
    let output = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .arg(root.join("runtime/ruby/active_support_ext.rb"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("contracts passed"),
        "to_query contract failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}
