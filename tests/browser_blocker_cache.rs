//! The CRuby overlay's memoized `allow_browser` verdict
//! (ruby_overlay/runtime/browser_blocker_cruby.rb) answers exactly what
//! the shared BrowserBlocker answers, for every agent and floors table,
//! and parses an agent once per thread rather than once per request.

use std::path::Path;
use std::process::Command;

#[test]
fn memoized_browser_verdicts_match_the_shared_blocker() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require_relative "runtime/ruby/action_controller/browser_blocker"
BB = ActionController::BrowserBlocker
shared = BB.method(:blocked?)
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/browser_blocker_cruby"

agents = [
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/114.0",
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2 Safari/605.1.15",
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
  "Mozilla/5.0 (Windows NT 10.0; Trident/7.0; rv:11.0) like Gecko",
  "Googlebot/2.1 (+http://www.google.com/bot.html)",
  "Roundhouse Test", "", nil,
]
campfire = -> { { "safari" => "17.2", "chrome" => "120", "firefox" => "121", "opera" => "104", "ie" => "false" } }
strict = -> { { "chrome" => "141", "firefox" => "200" } }
2.times do
  agents.each do |ua|
    [campfire, strict, -> { BB::MODERN }].each do |floors|
      got = BB.blocked?(ua, floors.())
      want = shared.call(ua, floors.())
      raise "#{ua.inspect} under #{floors.().inspect}: #{got} vs #{want}" unless got == want
    end
  end
end

# The same agent under two tables is two verdicts: Chrome 140 passes
# campfire's floor of 120 and is blocked by a floor of 141.
chrome = agents[2]
raise "tables crossed" unless BB.blocked?(chrome, campfire.()) == false && BB.blocked?(chrome, strict.()) == true

# A hit does not parse: the parser is not reached for a seen agent.
calls = 0
UserAgent.singleton_class.prepend(Module.new { define_method(:parse) { |*a| calls += 1; super(*a) } })
5.times { BB.blocked?(chrome, campfire.()) }
raise "a seen agent was parsed again (#{calls})" unless calls == 0
BB.blocked?(chrome + " extra/1", campfire.())
raise "a new agent was not parsed" unless calls == 1

# The table stays bounded: past the cap it is cleared, and verdicts stay right.
(BB::VERDICT_CACHE_CAP + 10).times { |i| BB.blocked?("Mozilla/5.0 Firefox/#{i}.0", campfire.()) }
tables = Thread.current.thread_variable_get(:roundhouse_browser_verdicts)
raise "unbounded: #{tables.values.map(&:size).max}" unless tables.values.all? { |t| t.size <= BB::VERDICT_CACHE_CAP }
raise "verdict after clear" unless BB.blocked?(agents[0], campfire.()) == true

# A caller mutating its agent string afterwards does not change the key.
ua = +"Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/300.0"
BB.blocked?(ua, campfire.())
ua.replace(agents[0])
raise "a mutated agent answered the old verdict" unless BB.blocked?(agents[0], campfire.()) == true
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
        "memoized browser verdicts failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
