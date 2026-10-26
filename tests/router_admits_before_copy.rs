//! `Router.scan_routes` (runtime/ruby/action_dispatch/router.rb) asks
//! the cheap shape test (`admits`: segment count and lead segment)
//! before copying the route's verb and parameter strings, so a match
//! no longer allocates once per route it walks past. The copies are a
//! Spinel workaround and stay; they are just made for the few routes
//! whose shape fits. Both tests are pure, so which route wins (verb
//! order, `via: :all`, Rails' two-pass HEAD rule, globs, formats) must
//! not move.

use std::path::Path;
use std::process::Command;

#[test]
fn shape_test_runs_before_the_copies_and_matches_are_unchanged() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require_relative "runtime/ruby/action_dispatch/router"
R = ActionDispatch::Router::Route
table = (1..80).map { |i| R.new("GET", "/r#{i}/:id", "c#{i}", "show") }
table << R.new("POST", "/rooms/:id/messages", "messages", "create")
table << R.new("GET", "/rooms/:id/messages", "messages", "index")
table << R.new("ANY", "/rooms/:id", "rooms", "any")
table << R.new("GET", "/rooms/:id", "rooms", "show")
table << R.new("HEAD", "/probe", "p", "head")
table << R.new("GET", "/probe", "p", "get")
table << R.new("GET", "/files/*path", "f", "glob")

def m(verb, path, table)
  r = ActionDispatch::Router.match(verb, path, table)
  r && [r.controller, r.action, r.path_params]
end
cases = {
  ["GET", "/rooms/1/messages"] => ["messages", "index", { "id" => "1" }],
  ["POST", "/rooms/1/messages"] => ["messages", "create", { "id" => "1" }],
  ["PUT", "/rooms/1/messages"] => nil,
  # `via: :all` is declared first, so it wins for GET, and for HEAD on
  # the GET pass; a verb nothing else claims also lands on it.
  ["GET", "/rooms/1"] => ["rooms", "any", { "id" => "1" }],
  ["HEAD", "/rooms/1"] => ["rooms", "any", { "id" => "1" }],
  ["DELETE", "/rooms/1"] => ["rooms", "any", { "id" => "1" }],
  ["GET", "/rooms/1.json"] => ["rooms", "any", { "id" => "1", "format" => "json" }],
  # A route declared for HEAD beats the GET it would otherwise shadow.
  ["HEAD", "/probe"] => ["p", "head", {}],
  ["GET", "/probe"] => ["p", "get", {}],
  ["GET", "/files/a/b/c"] => ["f", "glob", { "path" => "a/b/c" }],
  ["GET", "/r7/9"] => ["c7", "show", { "id" => "9" }],
  ["GET", "/nope"] => nil,
}
cases.each do |(verb, path), want|
  got = m(verb, path, table)
  raise "#{verb} #{path}: #{got.inspect}, want #{want.inspect}" unless got == want
end

# 80 routes of another shape sit ahead of the match. Copying each one's
# verb before the shape test cost 107 objects per match here; asking the
# shape first costs 27.
5.times { m("GET", "/rooms/1/messages", table) }
before = GC.stat(:total_allocated_objects)
1000.times { ActionDispatch::Router.match("GET", "/rooms/1/messages", table) }
per = (GC.stat(:total_allocated_objects) - before) / 1000.0
raise "#{per} objects per match (want <= 40)" unless per <= 40
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
        "router match failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
