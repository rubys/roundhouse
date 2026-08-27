# Driver for tests/shared_autolink.rs — the SHARED `auto_link`, the one
# every target compiles, loaded WITHOUT the CRuby overlay.
#
# The overlay redefines `auto_link` on top of the real `rails_autolink`
# chain, so `tests/overlay_sanitize_autolink.rb` — which loads both —
# cannot see this implementation at all. Every strict target can: it is
# the only `auto_link` they have, and campfire runs every message body
# through it.
#
# Every expected value below was MEASURED against `rails_autolink`
# 1.1.8 on `actionview` 8.1.3, by running the same input through the
# real helper. None was reasoned out.
$stdout.sync = true
ROOT = ARGV[0]
require "cgi"
require File.join(ROOT, "runtime/spinel/scaffold/ruby_overlay/runtime/active_support_core_ext")
require File.join(ROOT, "runtime/ruby/action_view/view_helpers")
require File.join(ROOT, "runtime/ruby/action_view/view_helpers_ext")

V = ActionView::ViewHelpers
fail_count = 0
check = lambda do |label, got, want|
  if got == want
    puts "ok   #{label}"
  else
    fail_count += 1
    puts "FAIL #{label}\n  want #{want.inspect}\n  got  #{got.inspect}"
  end
end
puts "shared auto_link, no overlay"

# ── The gem's LINKER, byte for byte ───────────────────────────────────
#
# These are `auto_link(..., :sanitize => false)` on the real gem, which
# is the gem MINUS its body-sanitize pass — and the body-sanitize pass
# is the one thing this port does not do (see the header of
# `runtime/ruby/action_view/view_helpers_ext.rb`). On that comparison
# the two agree on every probe below, which is the claim worth gating:
# the LINKING DECISIONS are the gem's, not an approximation of them.
check.("a plain url, with the caller's attribute first",
       V.auto_link("see https://example.com/a", html: { target: "_blank" }),
       %q{see <a target="_blank" href="https://example.com/a">https://example.com/a</a>})
check.("empty text is empty", V.auto_link(""), "")
check.("a url already inside an anchor is left alone",
       V.auto_link(%q{<a href="https://x.com">https://x.com</a>}),
       %q{<a href="https://x.com">https://x.com</a>})
check.("a url inside an ATTRIBUTE is left alone",
       V.auto_link(%q{<img alt="https://x.co/q">}), %q{<img alt="https://x.co/q">})
check.("the sentence's full stop is not the url's",
       V.auto_link("go https://example.com/a."),
       %q{go <a href="https://example.com/a">https://example.com/a</a>.})
# The bracket clause, which is the whole reason the punctuation strip is
# a loop and not a chomp.
check.("a bracket the url OPENED is kept",
       V.auto_link("see https://en.wikipedia.org/wiki/Foo_(bar) end"),
       %q{see <a href="https://en.wikipedia.org/wiki/Foo_(bar)">https://en.wikipedia.org/wiki/Foo_(bar)</a> end})
check.("a bracket the url did NOT open is dropped",
       V.auto_link("trailing bracket https://x.co/a)"),
       %q{trailing bracket <a href="https://x.co/a">https://x.co/a</a>)})
check.("`/`, `-`, `=` and `;` may end a url",
       V.auto_link("a https://x.co/a- b https://x.co/b= c https://x.co/c;"),
       %q{a <a href="https://x.co/a-">https://x.co/a-</a> b } +
       %q{<a href="https://x.co/b=">https://x.co/b=</a> c } +
       %q{<a href="https://x.co/c;">https://x.co/c;</a>})
check.("a trailing &gt; is the markup's, not the url's",
       V.auto_link("https://x.co/a&gt; then"),
       %q{<a href="https://x.co/a">https://x.co/a</a>&gt; then})
# `www.` gets the scheme in the HREF and keeps the text as written.
check.("www. is prefixed in the href and left alone in the text",
       V.auto_link("visit www.example.com now"),
       %q{visit <a href="http://www.example.com">www.example.com</a> now})
check.("the scheme list and `www.` are case-insensitive",
       V.auto_link("HTTPS://EX.CO/A and WWW.Example.COM/x"),
       %q{<a href="HTTPS://EX.CO/A">HTTPS://EX.CO/A</a> and } +
       %q{<a href="http://WWW.Example.COM/x">WWW.Example.COM/x</a>})
check.("an unlisted scheme is not a url",
       V.auto_link("not-a-scheme foo://bar.co/x"), "not-a-scheme foo://bar.co/x")
check.("a listed scheme is not matched by its SUFFIX",
       V.auto_link("sftp://files.co/y"),
       %q{<a href="sftp://files.co/y">sftp://files.co/y</a>})
check.("a `\"` ends a url", V.auto_link(%q{https://x.co/a"b}),
       %q{<a href="https://x.co/a">https://x.co/a</a>"b})
# The e-mail pass, and the anchor it goes through.
check.("an e-mail address becomes a mailto",
       V.auto_link("mail me at sam@rubyred.us ok"),
       %q{mail me at <a href="mailto:sam@rubyred.us">sam@rubyred.us</a> ok})
check.("an e-mail's own trailing dot is not part of it",
       V.auto_link("at sam.ruby+tag@sub.example.co.uk."),
       # `%2B`, not `+` — Rails percent-encodes the mailto address and
       # our `mail_to` agrees. Measured on both.
       %q{at <a href="mailto:sam.ruby%2Btag@sub.example.co.uk">sam.ruby+tag@sub.example.co.uk</a>.})
check.("an e-mail inside a linked url is not linked again",
       V.auto_link("see https://x.co/a@b.co ok"),
       %q{see <a href="https://x.co/a@b.co">https://x.co/a@b.co</a> ok})
check.("a one-label domain is not an e-mail",
       V.auto_link("not a@b here"), "not a@b here")
# `auto_linked?`'s first clause is CRUDER than "skip over tags", and
# this is the probe that tells the two apart: the character right after
# a `<` is not inside a tag, so the address is linked.
check.("the character after a `<` is not inside a tag",
       V.auto_link("addr <foo@bar.com> ok"),
       %q{addr <<a href="mailto:foo@bar.com">foo@bar.com</a>> ok})
check.("a `<` that never closes does not suppress linking",
       V.auto_link("unterminated <b tag https://x.co/y"),
       %q{unterminated <b tag <a href="https://x.co/y">https://x.co/y</a>})
check.("anchor text is skipped, text after `</a>` is not",
       V.auto_link(%q{<a href='x'>inner https://no.co/l</a> out https://yes.co/l}),
       %q{<a href='x'>inner https://no.co/l</a> out } +
       %q{<a href="https://yes.co/l">https://yes.co/l</a>})
check.("`<abbr>` is not an anchor",
       V.auto_link("<abbr>https://x.co/a</abbr>"),
       %q{<abbr><a href="https://x.co/a">https://x.co/a</a></abbr>})
# `:link` narrows what is linked.
check.("link: :urls leaves e-mail alone",
       V.auto_link("https://x.co/a and me@x.co", link: :urls),
       %q{<a href="https://x.co/a">https://x.co/a</a> and me@x.co})
check.("link: :email_addresses leaves urls alone",
       V.auto_link("https://x.co/a and me@x.co", link: :email_addresses),
       %q{https://x.co/a and <a href="mailto:me@x.co">me@x.co</a>})

# ── The campfire expression ───────────────────────────────────────────
#
# `auto_link h(...), html: { target: "_blank" }`, on a body that already
# went through `h` — which is why the `&` below is an entity before it
# ever reaches here, and why the port's skipped body-sanitize does not
# show up on this path.
check.("the message-body shape: markup kept, url linked, entity intact",
       V.auto_link("Deploy <b>3.4.10</b> — https://ex.co/a?x=1&amp;y=2",
                   html: { target: "_blank" }),
       %q{Deploy <b>3.4.10</b> — } +
       %q{<a target="_blank" href="https://ex.co/a?x=1&amp;y=2">} +
       %q{https://ex.co/a?x=1&amp;y=2</a>})

# ── The DIVERGENCE, pinned rather than described ──────────────────────
#
# Rails safe-list-sanitizes the body first and this does not (the pass
# is HTML5 tree construction, not filtering — see `sanitize` in the same
# file). These are the values where that shows, with the gem's DEFAULT
# answer in the comment. If a future change starts sanitizing, these are
# the assertions that will say so instead of the campfire suite.
check.("bare angle brackets survive (Rails: `a &lt; b &gt; c`)",
       V.auto_link("a < b > c"), "a < b > c")
check.("an unknown tag survives (Rails drops it: `addr  ok`)",
       V.auto_link("addr <foo@bar.com> ok"),
       %q{addr <<a href="mailto:foo@bar.com">foo@bar.com</a>> ok})
check.("single-quoted attributes are not renormalised (Rails: `href=\"x\"`)",
       V.auto_link("<a href='x'>t</a>"), "<a href='x'>t</a>")
# And the one that follows from it: with no body pass, a bare `&` is
# never turned into an entity, so it reaches the href as written.
check.("a bare `&` reaches the href as written (Rails: `&amp;`)",
       V.auto_link("https://x.co/a?b=1&c=2"),
       %q{<a href="https://x.co/a?b=1&c=2">https://x.co/a?b=1&c=2</a>})

# ── `sanitize:` is accepted and inert ─────────────────────────────────
#
# All three settings produce the same anchor in the gem too: `escape` is
# `content_tag`'s fourth argument and it is true only when the sanitize
# ran, at which point the value is html_safe and is spliced raw anyway.
same = V.auto_link("https://x.co/a?b=1&amp;c=2")
check.("sanitize: false is the same anchor",
       V.auto_link("https://x.co/a?b=1&amp;c=2", sanitize: false), same)
check.("sanitize: true is the same anchor",
       V.auto_link("https://x.co/a?b=1&amp;c=2", sanitize: true), same)

puts(fail_count.zero? ? "ALL OK" : "#{fail_count} FAILED")
exit(fail_count.zero? ? 0 : 1)
