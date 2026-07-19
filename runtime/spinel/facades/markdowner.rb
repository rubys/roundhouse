# Markdowner façade — lobsters' CommonMark renderer (app/models/
# markdowner.rb) walks a Markly DOM tree with a recursive block-driving
# helper: `walk_text_nodes` forwards its `&block` through
# `Markly::Node#each` and calls itself. Under spinel AOT that
# identity-forwarding block-through-a-yielding-method recursion is
# refused (matz/spinel#2948 — a deliberate always-inline boundary), so
# the verbatim app body cannot compile. Every consumer is WRITE-PATH:
# the `markeddown_*` columns are precomputed on save (mod_note,
# invitation_request, message, comment, story, user), so the read
# benchmark never renders markdown. The scaffold base ships this raising
# stand-in at the same emit path (`app/models/markdowner.rb`), leaving
# the require graph untouched; the CRuby tree — where Markly is real and
# the recursion runs as written — restores the verbatim emit (see
# emit::ruby::library::restore_extras_facades). Same raise-loudly
# contract as runtime/ruby/gem_facades.rb, which already houses the
# Markly/Nokogiri façades this body would otherwise drive.
#
# The real fix, when a target renders markdown at REQUEST time (Mastodon,
# not lobsters' read benchmark), is a Commonmarker façade over the gem's
# iterative `Node#walk` — the "enabled shape" that expresses the traversal
# as flat iteration spinel already compiles, with no recursive block.
require_relative "../../runtime/gem_facades"

class Markdowner
  def self.to_html(text, opts = {})
    GemFacade.fail!("Markdowner.to_html")
    ""
  end
end
