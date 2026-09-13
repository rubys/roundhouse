# `ActionController::AllowBrowser::BrowserBlocker` (actionpack 8.1), the
# question behind `allow_browser versions: …`: is this request's browser
# one the app gates, and below the floor it set?
#
#   blocked?  =  the User-Agent reports a version
#             && the browser is in the floors table
#             && (the floor is `false` || reported < floor)
#             && the agent is not a bot
#
# `floors` is the app's `versions:` hash with every key and value a
# String — `{ "safari" => "17.2", "chrome" => "120", "ie" => "false" }`
# — which is what the lowering hands over from the literal the app
# wrote (or from `:modern`, Rails' own table, `MODERN` below). Strings
# rather than the Float/Integer/false Rails accepts because the floor
# is only ever compared as a version (`UserAgent.version_below?`), and
# a table of one type is a shape every target's emit answers.
#
# Rails normalizes the parsed browser name to lowercase and folds
# "internet explorer" to "ie" so the app can write `ie: false`.
require_relative "../user_agent"

module ActionController
  module BrowserBlocker
    # actionpack's `SETS[:modern]` at 8.1.
    MODERN = { "safari" => "17.2", "chrome" => "120", "firefox" => "121", "opera" => "106", "ie" => "false" }.freeze

    def self.blocked?(user_agent, floors)
      return false if user_agent.nil? || user_agent.empty?
      parsed = UserAgent.parse(user_agent)
      reported = parsed.version
      return false if reported.empty?
      name = parsed.browser.downcase
      name = "ie" if name == "internet explorer"
      floor = floors[name]
      return false if floor.nil?
      return false if parsed.bot?
      floor == "false" || UserAgent.version_below?(reported, floor)
    end
  end
end
