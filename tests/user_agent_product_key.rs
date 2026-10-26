//! `UserAgent.product?` / `find_product` compare product names
//! case-insensitively against a key each token lowercases once when it
//! is parsed (runtime/ruby/user_agent.rb), instead of lowercasing both
//! sides on every comparison. A page asks again for every browser
//! predicate — campfire's PWA partials call `platform.chrome?`,
//! `firefox?`, `safari?`… a dozen times per render, and on a Gecko agent
//! each `browser` walks every `GECKO_BROWSERS` name against every token.
//!
//! The answers must not move: the table below was produced by the
//! previous implementation over the same agents.

use std::path::Path;
use std::process::Command;

#[test]
fn product_lookup_answers_unchanged_and_allocates_once_per_name() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require_relative "runtime/ruby/user_agent"

# [agent, browser, version, platform, os, bot?] from the implementation
# that lowercased both sides on every comparison.
EXPECTED = [
  ["Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/114.0", "Firefox", "114.0", "Windows", "Windows 10", false],
  ["Mozilla/5.0 (X11; Linux x86_64; rv:102.0) Gecko/20100101 Goanna/6.2 Firefox/102.0 PaleMoon/32.3.1", "PaleMoon", "32.3.1", "X11", "Linux x86_64", false],
  ["Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2 Safari/605.1.15", "Safari", "17.2", "Macintosh", "OS X 10.15.7", false],
  ["Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36", "Chrome", "140.0.0.0", "Macintosh", "OS X 10.15.7", false],
  ["Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/120.0.6099.119 Mobile/15E148 Safari/604.1", "Chrome", "120.0.6099.119", "iPhone", "iOS 17.0", false],
  ["Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Mobile Safari/537.36 EdgA/120.0", "Chrome", "120.0", "Android", "Android 14", false],
  ["Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0", "Chrome", "120.0.0.0", "Windows", "Windows 10", false],
  ["Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Vivaldi/6.5", "Vivaldi", "5.0", "", "", false],
  ["Mozilla/5.0 (Windows NT 10.0; Trident/7.0; rv:11.0) like Gecko", "Internet Explorer", "11.0", "Windows", "Windows 10", false],
  ["Opera/9.80 (Windows NT 6.1) Presto/2.12.388 Version/12.16", "Opera", "9.80", "", "", false],
  ["Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36 OPR/106.0", "Opera", "5.0", "", "", false],
  ["Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36 Chrome-Lighthouse", "Chrome", "120.0.0.0", "Android", "Android 10", true],
  ["Googlebot/2.1 (+http://www.google.com/bot.html)", "Googlebot", "2.1", "", "", true],
  ["Mozilla/5.0 (X11; U; Linux i686; en-US; rv:1.8.1.4) Gecko/20070531 Camino/1.5", "Camino", "1.5", "X11", "Linux i686", false],
  ["Mozilla/5.0 (X11; Linux x86_64; rv:60.0) Gecko/20100101 ICEWEASEL/60.0", "Iceweasel", "60.0", "X11", "Linux x86_64", false],
  ["Roundhouse Test", "Roundhouse", "", "", "", false],
  ["", "Mozilla", "4.0", "", "", false],
]
EXPECTED.each do |s, *want|
  ua = UserAgent.parse(s)
  got = [ua.browser, ua.version.to_s, ua.platform.to_s, ua.os.to_s, ua.bot?]
  raise "#{s.inspect}: #{got.inspect} vs #{want.inspect}" unless got == want
end

# Case-insensitive, as the gem is: `detect_product("CriOs")` finds `CriOS`.
tokens = UserAgent.parse(EXPECTED[4][0]).instance_variable_get(:@tokens)
raise "case-insensitive product?" unless UserAgent.product?(tokens, "CriOs") && UserAgent.product?(tokens, "crios")
raise "case-insensitive find_product" unless UserAgent.find_product(tokens, "SAFARI")&.product == "Safari"
raise "absent product" if UserAgent.product?(tokens, "Firefox") || UserAgent.find_product(tokens, "Firefox")

# The allocation half. A Gecko agent's `browser` walks GECKO_BROWSERS
# against every token: lowercasing both sides on each comparison cost 14
# objects per call here; one key per searched name costs 4.
firefox = UserAgent.parse(EXPECTED[0][0])
10.times { firefox.browser }
before = GC.stat(:total_allocated_objects)
100.times { firefox.browser }
per_call = (GC.stat(:total_allocated_objects) - before) / 100.0
raise "browser allocates #{per_call} objects per call (want <= 6)" unless per_call <= 6
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
        "user-agent product lookup failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
