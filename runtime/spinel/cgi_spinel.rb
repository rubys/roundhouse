# Spinel-only CGI reopen. `require "cgi"` reaches spinel's bundled
# package (matz/spinel#4812, merged 2026-09-22 as fc714b2c); this file
# adds the ONE method that package does not carry.
#
# WHAT CHANGED, AND WHY IT IS A FIX RATHER THAN A TIDY-UP. `CGI.escape`
# used to route here to tep's `Url.escape`, which walks CHARACTERS and
# percent-encodes `c.bytes[0]` — the FIRST byte of the character. On
# anything outside ASCII that is data loss, not a formatting
# difference: "café" escaped to "caf%C3" and an emoji to "%F0", and
# neither unescapes back to what went in. Lobsters renders
# `CGI.escape(story.url)` on every front-page archive link. It also
# wrote a space as "%20" where `CGI.escape` writes "+", so the bytes
# differed from the ones Rails puts in the same attribute. The package
# is byte-for-byte CRuby's on both counts — its test is 35 assertions
# whose .expected is CRuby's own output.
#
# `CGI.parse` IS NOT IN THE PACKAGE, and that is upstream's shape
# rather than a gap: CRuby split the library, and on Ruby 4.0
# `require "cgi"` answers with the escape/unescape surface while the
# request object and `parse` moved to `cgi/core`. So `parse` is
# reopened here, the same arrangement `runtime/spinel/net_http.rb` has
# over `packages/net` — the library's own class, with the one addition
# the corpus needs on top.
#
# A `class`, NOT a `module`: the package declares `class CGI` (CRuby
# does too), and reopening it as a module is the collision
# `runtime/spinel/erb_spinel.rb`'s header records — it cost the
# lobsters AOT lane ten days.
#
# NOT NAMED `cgi.rb`: this file substitutes for a stdlib library on
# spinel ONLY — the CRuby/JRuby boot reaches the real one with a bare
# `require "cgi"` (ruby_overlay/boot.rb), whose `parse` handles
# repeated and valueless keys this one does not. `walk_dir_flat`
# copies every runtime/spinel/*.rb into EVERY target tree, so under
# the library's own name this file would answer that bare require
# whenever `runtime/` is on `$LOAD_PATH` — which the emitted Rakefile
# does (`t.libs << "runtime"`) — handing CRuby this reopen instead of
# the stdlib. See runtime/json_impl.rb for the same rule.
require "cgi"

class CGI
  # `CGI.parse(qs)` -> key -> [values] (the CGI contract; extras/github
  # reads `ps["access_token"].first`). Off-bench; values URL-decoded.
  # One value per key, and a pair with no "=" is skipped — narrower
  # than the stdlib's, which is why this is not named `cgi.rb`.
  def self.parse(qs)
    out = {}
    pairs = qs.split("&")
    i = 0
    while i < pairs.length
      pair = pairs[i]
      eq = pair.index("=")
      unless eq.nil?
        k = CGI.unescape(pair[0, eq])
        v = CGI.unescape(pair[(eq + 1), pair.length - eq - 1])
        out[k] = [v]
      end
      i += 1
    end
    out
  end
end
