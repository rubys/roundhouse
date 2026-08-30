# The `useragent` + `platform_agent` port, user-agent string by
# user-agent string.
#
# A FRAMEWORK test (`tests/runtime_ruby_unit.rs` runs every file under
# `runtime/ruby/test/` on each `cargo test`), not one that ships into an
# emitted tree.
#
# WHY THIS FILE EXISTS, and it is the IPAddr port's reason. A port is
# exactly the thing that fails quietly: a browser classified wrong
# renders the wrong PWA instructions and nothing raises. So the
# expectations are not written here — they are the REAL GEMS' answers,
# in `fixtures/user_agents.txt`, one row per string:
#
#   ua|browser|platform|os|version|ios?|android?|mac?|chrome?|firefox?|
#     safari?|edge?|apple_messages?|mobile?|desktop?|windows?|
#     operating_system
#
# The last twelve columns are campfire's `ApplicationPlatform`, which is
# what actually reaches the views — the port has to agree about the
# derived predicates, not just the four raw readers.
#
# REGENERATE, never hand-edit, with the gems installed:
#
#     require "active_support"
#     require "active_support/core_ext/module/delegation"
#     require "user_agent"
#     require "platform_agent"
#     # …paste campfire's ApplicationPlatform…
#     # …then print the row for each ua…
#
# THE TWO ROWS THAT WOULD HAVE BEEN WRITTEN WRONG BY HAND, and the
# argument for generating the file at all:
#
#   * modern Edge (`Edg/140.0.0.0`) reports its browser as **Chrome**.
#     `Browsers::Edge.extend?` tests `product == "Edge"` and the modern
#     token is `Edg`, so campfire's `edge?` is FALSE on current Edge.
#   * Firefox on iOS (`FxiOS/126.0`) reports **Safari**, because iOS
#     Firefox is WebKit and `Browsers::Webkit` claims it first, so
#     campfire's `firefox?` is FALSE there.
#
# Both are the gems' behaviour, so both are campfire's behaviour on
# Rails, and reproducing them is the whole point of a port.
require_relative "test_helper"
require_relative "../user_agent"

# campfire's `app/models/application_platform.rb`, verbatim — the
# subclass is what the fixture's derived columns were generated from,
# and it is also the only exercise `PlatformAgent`'s private `match?`
# and `user_agent` get.
class ApplicationPlatformFixture < PlatformAgent
  def ios?
    match?(/iPhone|iPad/)
  end

  def android?
    match?(/Android/)
  end

  def mac?
    match?(/Macintosh/)
  end

  def chrome?
    user_agent.browser.match?(/Chrome/)
  end

  def firefox?
    user_agent.browser.match?(/Firefox|FxiOS/)
  end

  def safari?
    user_agent.browser.match?(/Safari/)
  end

  def edge?
    user_agent.browser.match?(/Edg/)
  end

  def apple_messages?
    match?(/facebookexternalhit/i) && match?(/Twitterbot/i)
  end

  def mobile?
    ios? || android?
  end

  def desktop?
    !mobile?
  end

  def windows?
    operating_system == "Windows"
  end

  def operating_system
    case user_agent.platform
    when /Android/   then "Android"
    when /iPad/      then "iPad"
    when /iPhone/    then "iPhone"
    when /Macintosh/ then "macOS"
    when /Windows/   then "Windows"
    when /CrOS/      then "ChromeOS"
    else
      os =~ /Linux/ ? "Linux" : os
    end
  end
end

class UserAgentTest < Minitest::Test
  ROWS = File.readlines(
    File.expand_path("fixtures/user_agents.txt", __dir__), chomp: true
  ).reject(&:empty?).map { |line| line.split("|", -1) }

  def test_the_fixture_is_not_empty
    # A fixture that failed to load would make every assertion below
    # vacuous, and a green run would mean nothing.
    assert_operator ROWS.length, :>=, 26, "fixture rows"
  end

  def test_each_user_agent_matches_the_gems
    ROWS.each do |row|
      ua = row[0]
      agent = UserAgent.parse(ua)
      platform = ApplicationPlatformFixture.new(ua)
      actual = [
        agent.browser.to_s, agent.platform.to_s, agent.os.to_s,
        agent.version.to_s,
        platform.ios?, platform.android?, platform.mac?,
        platform.chrome?, platform.firefox?, platform.safari?,
        platform.edge?, platform.apple_messages?, platform.mobile?,
        platform.desktop?, platform.windows?,
        platform.operating_system.to_s
      ].map(&:to_s)
      assert_equal row[1..], actual, "for #{ua}"
    end
  end

  # An empty or nil string is the gem's `DEFAULT_USER_AGENT`, not a
  # crash — campfire calls `ApplicationPlatform.new(request.user_agent)`
  # and a request can carry no User-Agent header at all.
  def test_a_missing_user_agent_parses_as_the_default
    [nil, "", "   "].each do |ua|
      agent = UserAgent.parse(ua)
      assert_equal "Mozilla", agent.browser, "browser for #{ua.inspect}"
      assert_equal "4.0", agent.version, "version for #{ua.inspect}"
    end
    assert_equal false, ApplicationPlatformFixture.new(nil).ios?
  end
end
