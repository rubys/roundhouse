# `useragent` 0.16.11 and `platform_agent` 1.0.1, ported.
#
# campfire reaches these on EVERY room page, which is not what the
# façade they replace assumed. `app/views/rooms/involvements/_bell
# .html.erb` renders `pwa/_browser_settings`, `pwa/_system_settings` and
# `pwa/_install_instructions`, and each of those asks
# `ApplicationPlatform` which browser and which OS. The stub raised
# `NotImplementedError` from `PlatformAgent.new`, so `/rooms/1` was a
# 500 on the spinel binary — the first page after sign-in.
#
# PORTED, NOT DERIVED (see the IPAddr port for the same rule). The
# classification rules below are the gems' own, transcribed, and the
# suite beside this file compares them against the real gems over a
# user-agent corpus rather than against hand-written expectations. Two
# results are surprising enough that deriving them would have got them
# wrong:
#
#   * a MODERN Edge (`Edg/140.0.0.0`) reports its browser as **Chrome**.
#     `Browsers::Edge.extend?` tests `product == "Edge"`, and the modern
#     token is `Edg`. So campfire's `edge?` is false on current Edge and
#     true only on the legacy `Edge/13.10586` spelling.
#   * Firefox on iOS (`FxiOS/126.0`) reports **Safari**, because iOS
#     Firefox is WebKit and `Browsers::Webkit` claims it first.
#
# Both are the gem's behaviour, so both are campfire's behaviour on
# Rails, and reproducing them is the point.
#
# NO `method_missing`. The gems lean on it hard — `Browsers::Base`
# answers any product name as a method, and `Chrome#browser` is
# `ChromeBrowsers.detect { |b| respond_to?(b) }`. That is unreachable
# for a static target (see the runtime's own rule) and it is also just
# a product lookup, so it is spelled as one here.
#
# NOT an `Array` subclass either, for the same reason: the gem's
# `Browsers::Base < Array` is how `detect` reaches the tokens, and a
# subclass of a builtin container is a shape the strict targets do not
# carry.

# One `Product/Version (comment; comment)` group of a user-agent string.
class UserAgentToken
  def initialize(product, version, comment)
    @product = product
    @version = version.nil? || version.empty? ? "" : version
    @comment = comment.nil? ? [] : comment.split("; ")
  end

  def product
    @product
  end

  def version
    @version
  end

  # The parenthesised comment, split on `; ` — `["Macintosh", "Intel
  # Mac OS X 10_15_7"]`. Empty, never nil: the gem distinguishes "no
  # comment" from "empty comment" only in `application`, which is
  # spelled against `empty?` here.
  def comment
    @comment
  end
end

class UserAgent
  # `useragent`'s own MATCHER, with its `^` written `\A` (a user-agent
  # header is one line) and the outer alternation made non-capturing so
  # the comment is group 3.
  MATCHER = /\A['"]*([^\/\s]+)\/?([^\s,]*)(?:\s\(([^\)]*)\)|,gzip\(gfe\))?/

  DEFAULT_USER_AGENT = "Mozilla/4.0 (compatible)"

  GECKO_BROWSERS = [ "PaleMoon", "Firefox", "Camino", "Iceweasel", "Seamonkey" ]

  # `Browsers::Webkit::BuildVersions` — the WebKit build → Safari
  # version table for the releases before Safari 3, which shipped no
  # `Version/` token. Transcribed whole; a missing row is a blank
  # version, which is what the gem answers too.
  BUILD_VERSIONS = {
    "85.7" => "1.0", "85.8.5" => "1.0.3", "85.8.2" => "1.0.3",
    "124" => "1.2", "125.2" => "1.2.2", "125.4" => "1.2.3",
    "125.5.5" => "1.2.4", "125.5.6" => "1.2.4", "125.5.7" => "1.2.4",
    "312.1.1" => "1.3", "312.1" => "1.3", "312.5" => "1.3.1",
    "312.5.1" => "1.3.1", "312.5.2" => "1.3.1", "312.8" => "1.3.2",
    "312.8.1" => "1.3.2", "412" => "2.0", "412.6" => "2.0",
    "412.6.2" => "2.0", "412.7" => "2.0.1", "416.11" => "2.0.2",
    "416.12" => "2.0.2", "417.9" => "2.0.3", "418" => "2.0.3",
    "418.8" => "2.0.4", "418.9" => "2.0.4", "418.9.1" => "2.0.4",
    "419" => "2.0.4", "425.13" => "2.2", "534.52.7" => "5.1.2"
  }

  # `OperatingSystems::Windows` — the NT build → marketing name table.
  WINDOWS_NAMES = {
    "Windows NT 10.0" => "Windows 10", "Windows NT 6.3" => "Windows 8.1",
    "Windows NT 6.2" => "Windows 8", "Windows NT 6.1" => "Windows 7",
    "Windows NT 6.0" => "Windows Vista",
    "Windows NT 5.2" => "Windows XP x64 Edition",
    "Windows NT 5.1" => "Windows XP",
    "Windows NT 5.01" => "Windows 2000, Service Pack 1 (SP1)",
    "Windows NT 5.0" => "Windows 2000", "Windows NT 4.0" => "Windows NT 4.0",
    "Windows 98" => "Windows 98", "Windows 95" => "Windows 95",
    "Windows CE" => "Windows CE"
  }

  IOS_VERSION_REGEX = /CPU (?:iPhone |iPod )?OS ([\d_]+) like Mac OS X/

  def initialize(tokens, kind)
    @tokens = tokens
    @kind = kind
  end

  # Tokenize, then classify once. The gem re-derives the browser family
  # on every reader through `Browsers.extend`; the family is a fact
  # about the string, so it is computed here and carried.
  def self.parse(string)
    s = string.nil? ? "" : string.strip
    s = DEFAULT_USER_AGENT if s.empty?
    tokens = []
    # Read through `String#[]` rather than the gem's `MatchData`, and a
    # `while` rather than its `loop do … break`. Same walk — consume the
    # leading `Product/Version (comment)` group, then rescan what is
    # left — but every value in it has a type, and `runtime/ruby/` is
    # priced for every target by exactly that
    # (`tests/runtime_src_integration.rs`, whose registry is built from
    # the `.rbs` files and so knows no `MatchData`). The `loop` +
    # `MatchData` spelling left 21 untyped sub-expressions in this one
    # method. Four scans instead of one, on a string of about a dozen
    # tokens, once per request.
    matched = s[MATCHER, 0]
    while !matched.nil? && !matched.empty?
      tokens << UserAgentToken.new(
        s[MATCHER, 1].to_s, s[MATCHER, 2].to_s, s[MATCHER, 3]
      )
      rest = s[matched.length, s.length - matched.length]
      s = rest.nil? ? "" : rest.strip
      break if s.empty?
      matched = s[MATCHER, 0]
    end
    new(tokens, classify(tokens))
  end

  # `Browsers::ALL`, in its order — first match wins, and the order is
  # load-bearing (Chrome ahead of Webkit is why a Chrome UA, which also
  # carries `AppleWebKit`, is not "Safari").
  #
  # The families this port DOES NOT carry — WechatBrowser, ITunes,
  # PlayStation, PodcastAddict, WindowsMediaPlayer, AppleCoreMedia,
  # Libavformat — fall through to the ones below them, exactly as they
  # would if the gem's list were this list. They are media players and
  # embedded browsers no corpus app branches on, and adding one is
  # adding a row here plus a row in the comparison corpus.
  def self.classify(tokens)
    return "base" if tokens.empty?
    last = tokens.last
    # Read the product ONCE, here, rather than re-testing `last` below.
    # `return "edge" if !last.nil? && last.product == "Edge"` narrows
    # `last` to `Nil` on the fall-through path — the negation of
    # `!last.nil? && product == "Edge"` is a DISJUNCTION, and being
    # nil is only one of its arms — so a later `last.product` reads off
    # a receiver analysis believes is nil. Ledgered in
    # docs/pipeline/analyze.md; hoisting the read is correct either way.
    last_product = last.nil? ? "" : last.product
    return "edge" if last_product == "Edge"
    app = base_application(tokens)
    if !app.nil? && !app.comment.empty?
      second = app.comment[1]
      joined = app.comment.join("; ")
      if (!second.nil? && second.match?(/MSIE/)) || joined.match?(/Trident.+rv:/)
        return "internet_explorer"
      end
    end
    first = tokens.first
    first_product = first.nil? ? "" : first.product
    app_product = app.nil? ? "" : app.product
    if first_product == "Opera" || app_product == "Opera" || last_product == "OPR"
      return "opera"
    end
    return "vivaldi" if product?(tokens, "Vivaldi")
    return "chrome" if product?(tokens, "Chrome") || product?(tokens, "CriOS")
    return "webkit" if webkit?(tokens)
    return "gecko" if !app.nil? && app.product == "Mozilla"
    "base"
  end

  def self.webkit?(tokens)
    tokens.each do |t|
      return true if t.product.match?(/\AAppleWebKit\z/i)
      t.comment.each do |c|
        return true if c.match?(/\A(AppleWebKit)\/([\d\.]+)/i)
      end
    end
    false
  end

  # `detect_product` — the gem compares product names case-INSENSITIVELY
  # (which is why its own `detect_product("CriOs")` finds `CriOS`).
  def self.product?(tokens, name)
    tokens.each { |t| return true if t.product.downcase == name.downcase }
    false
  end

  def self.find_product(tokens, name)
    tokens.each { |t| return t if t.product.downcase == name.downcase }
    nil
  end

  def self.base_application(tokens)
    tokens.first
  end

  # `Chrome#application` and `Webkit#application` both skip tokens with
  # no comment — the browser token itself carries none, and the OS lives
  # in the leading `Mozilla/5.0 (…)` group.
  def self.commented_application(tokens)
    tokens.each { |t| return t unless t.comment.empty? }
    nil
  end

  def application
    if @kind == "chrome" || @kind == "webkit"
      UserAgent.commented_application(@tokens)
    else
      UserAgent.base_application(@tokens)
    end
  end

  def browser
    app = application
    case @kind
    when "edge" then "Edge"
    when "internet_explorer" then "Internet Explorer"
    when "opera" then "Opera"
    when "vivaldi" then "Vivaldi"
    when "chrome" then "Chrome"
    when "webkit"
      if os.match?(/Android/)
        "Android"
      elsif platform == "BlackBerry"
        platform
      else
        "Safari"
      end
    when "gecko"
      named = gecko_browser
      named.empty? ? (app.nil? ? "" : app.product) : named
    else
      app.nil? ? "" : app.product
    end
  end

  def gecko_browser
    GECKO_BROWSERS.each do |name|
      return name if UserAgent.product?(@tokens, name)
    end
    ""
  end

  def version
    app = application
    case @kind
    when "edge"
      last = @tokens.last
      last.nil? ? "" : last.version
    when "internet_explorer"
      return "" if app.nil?
      app.comment.join("; ")[/(MSIE\s|rv:)([\d\.]+)/, 2].to_s
    when "chrome"
      t = UserAgent.find_product(@tokens, "CriOS")
      t = UserAgent.find_product(@tokens, "Chrome") if t.nil?
      t.nil? ? "" : t.version
    when "webkit"
      webkit_version
    when "gecko"
      named = gecko_browser
      t = named.empty? ? nil : UserAgent.find_product(@tokens, named)
      v = t.nil? ? "" : t.version
      v.empty? ? (app.nil? ? "" : app.version) : v
    else
      app.nil? ? "" : app.version
    end
  end

  # Safari's version comes from a `Version/` token; before Safari 3
  # there was none and the WebKit build number stands in for it.
  def webkit_version
    t = UserAgent.find_product(@tokens, "Version")
    return t.version unless t.nil?
    ios = os[/iOS ([\d\.]+)/, 1].to_s
    return ios.tr("_", ".") if !ios.empty? && browser == "Safari"
    build = webkit_build
    v = BUILD_VERSIONS[build]
    v.nil? ? "" : v
  end

  def webkit_build
    t = UserAgent.find_product(@tokens, "AppleWebKit")
    return t.version unless t.nil?
    @tokens.each do |tok|
      tok.comment.each do |c|
        v = c[/\A(AppleWebKit)\/([\d\.]+)/i, 2].to_s
        return v unless v.empty?
      end
    end
    ""
  end

  # The gem answers nil where no platform is known; this answers "",
  # which every caller treats identically — `nil =~ /Android/` and
  # `"" =~ /Android/` are both no-match — and which lets the signature
  # stay `String` for the targets that need one.
  def platform
    app = application
    return "" if app.nil?
    case @kind
    when "edge", "internet_explorer" then "Windows"
    when "chrome"
      return "" if app.comment.empty?
      first = app.comment[0]
      return "Windows" if !first.nil? && first.match?(/Windows/)
      return "ChromeOS" if app.comment.any? { |c| c.match?(/CrOS/) }
      return "Android" if app.comment.any? { |c| c.match?(/Android/) }
      first.nil? ? "" : first
    when "webkit"
      return "" if app.comment.empty?
      first = app.comment[0]
      return "Windows" if !first.nil? && first.match?(/Windows/)
      return "BlackBerry" if first == "BB10"
      return "Android" if app.comment.any? { |c| c.match?(/Android/) }
      first.nil? ? "" : first
    when "gecko"
      return "" if app.comment.empty?
      first = app.comment[0]
      return "" if first == "compatible" || first == "Mobile"
      return "Windows" if !first.nil? && first.match?(/\AWindows /)
      first.nil? ? "" : first
    else
      ""
    end
  end

  def os
    app = application
    return "" if app.nil?
    case @kind
    when "edge"
      UserAgent.normalize_os(comment_match(/Windows NT [\d\.]+|Windows Phone (OS )?[\d\.]+/))
    when "internet_explorer"
      UserAgent.normalize_os(
        app.comment.join("; ")[/Windows NT [\d\.]+|Windows Phone (OS )?[\d\.]+/, 0].to_s
      )
    when "chrome"
      chrome_os(app)
    when "webkit"
      webkit_os(app)
    when "gecko"
      gecko_os(app)
    else
      ""
    end
  end

  def chrome_os(app)
    return "" if app.comment.empty?
    first = app.comment[0]
    return UserAgent.normalize_os(first) if !first.nil? && first.match?(/Windows NT/)
    second = app.comment[1]
    third = app.comment[2]
    return UserAgent.normalize_os(second.nil? ? "" : second) if third.nil?
    return UserAgent.normalize_os(second) if !second.nil? && second.match?(/Android/)
    UserAgent.normalize_os(third)
  end

  def webkit_os(app)
    return "" if app.comment.empty?
    first = app.comment[0]
    return UserAgent.normalize_os(first) if !first.nil? && first.match?(/Windows NT/)
    second = app.comment[1]
    third = app.comment[2]
    return UserAgent.normalize_os(second.nil? ? "" : second) if third.nil?
    return UserAgent.normalize_os(second) if !second.nil? && second.match?(/Android/)
    ios = app.comment.find { |c| c.match?(IOS_VERSION_REGEX) }
    return UserAgent.normalize_os(ios) unless ios.nil?
    UserAgent.normalize_os(third)
  end

  # Gecko picks the comment slot by what sits in it: the security flag
  # (`U`) pushes the OS one further along, and a leading `Windows ` or
  # `Android` puts it first.
  def gecko_os(app)
    return "" if app.comment.empty?
    first = app.comment[0]
    second = app.comment[1]
    lead = first.nil? ? "" : first
    index = if second == "U"
      2
    elsif lead.match?(/\AWindows /) || lead.match?(/\AAndroid/)
      0
    elsif lead == "Mobile"
      -1
    else
      1
    end
    return "" if index < 0
    slot = app.comment[index]
    UserAgent.normalize_os(slot.nil? ? "" : slot)
  end

  def comment_match(pattern)
    @tokens.each do |t|
      t.comment.each do |c|
        hit = c[pattern, 0].to_s
        return hit unless hit.empty?
      end
    end
    ""
  end

  # `OperatingSystems.normalize_os` — the Windows table first, then the
  # three version-bearing families, then the string as written.
  def self.normalize_os(os)
    return "" if os.nil? || os.empty?
    named = WINDOWS_NAMES[os]
    return named unless named.nil?
    # `String#[](regex, capture)` rather than `match` + `MatchData`:
    # the analyzer types `match` as gradual on purpose ("we don't model
    # MatchData structurally"), and every read off it was a
    # `Ty::Untyped` site in a directory that prices every target.
    #
    # Mac needs BOTH reads because its version group is OPTIONAL — a
    # bare `Intel Mac OS X` matches with no capture, and "matched with
    # an empty capture" and "did not match" are different answers. The
    # iOS and ChromeOS groups are mandatory, so a non-empty capture is
    # itself the proof that the pattern hit.
    unless os[/(?:Intel|PPC) Mac OS X\s*([0-9_\.]+)?/, 0].to_s.empty?
      v = os[/(?:Intel|PPC) Mac OS X\s*([0-9_\.]+)?/, 1].to_s
      return v.empty? ? "OS X" : "OS X #{v.tr('_', '.')}"
    end
    v = os[IOS_VERSION_REGEX, 1].to_s
    return "iOS #{v.tr('_', '.')}" unless v.empty?
    v = os[/CrOS\s([^\s]+)\s(\d+(\.\d+)*)/, 2].to_s
    return "ChromeOS #{v}" unless v.empty?
    os
  end
end

# `platform_agent` 1.0.1. campfire subclasses this as
# `ApplicationPlatform` and builds every predicate the PWA partials ask
# for on top of `match?`, `user_agent` and `os`.
#
# `match?` and `user_agent` are PRIVATE in the gem and private here —
# the subclass calls both with an implicit receiver, which is the only
# spelling that works either way, and a public copy would say the gem
# offers a surface it does not.
class PlatformAgent
  def initialize(user_agent_string)
    @user_agent_string = user_agent_string
    @user_agent = UserAgent.parse(user_agent_string)
  end

  # `delegate :browser, :version, :product, :os, to: :user_agent` in the
  # gem. Three of the four are here; `product` is not, deliberately.
  #
  # `product` only LOOKS like a reader. `Browsers::Base` defines no such
  # method, so the delegation lands in the gem's `method_missing`, which
  # reads the name as a product to search for and answers the token
  # whose product is literally `"product"` — nil, for every real string.
  # Porting an accident of `method_missing` as if it were a reader would
  # be modelling the bug rather than the gem; a caller that wants the
  # product asks `user_agent.application`.
  #
  # campfire reaches `browser` from all three PWA partials
  # (`platform.browser.capitalize`) and `version` from none of them —
  # `version` is here because it is the same delegation and leaving it
  # out would be an arbitrary line.
  def browser
    @user_agent.browser
  end

  def version
    @user_agent.version
  end

  def os
    @user_agent.os
  end

  private
    def match?(pattern)
      s = @user_agent_string
      s.nil? ? false : s.match?(pattern)
    end

    def user_agent
      @user_agent
    end
end
