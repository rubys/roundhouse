# Driver for tests/shared_sanitize.rs — the SHARED safe-list sanitizer,
# the one every target compiles, loaded WITHOUT the CRuby overlay.
#
# The overlay redefines `sanitize` / `sanitize_allowing` on top of the
# real `rails-html-sanitizer` gem, so the CRuby lane never executes this
# implementation. Every strict target does: it is the only safe-list
# sanitizer they have, and campfire runs every message body through it
# (`ContentFilters::SanitizeAttributes` → `sanitize_allowing`).
#
# Every expected value below was MEASURED against
# `Rails::HTML5::SafeListSanitizer` (rails-html-sanitizer 1.7.1,
# loofah 2.25.2 — campfire's own lock), by running the same input with
# the same allow-lists through the real gem. The checks marked
# `DIVERGES` carry the port's declared-policy answer with the gem's
# beside them; the policies are named in the header of
# `runtime/ruby/action_view/view_helpers_ext.rb` and ledgered in
# docs/pipeline/runtime.md. Regenerate the block against a newer gem
# with the oracle recipe in that header's test note.
$stdout.sync = true
ROOT = ARGV[0]
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
puts "shared sanitize, no overlay"

# campfire's own allow-lists, verbatim: `ContentFilters::SanitizeTags::
# ALLOWED_TAGS` (+ the attachment tag names), and the gem's default
# attributes + `ActionText::Attachment::ATTRIBUTES` + `class`.
CAMPFIRE_TAGS = %w[ a abbr acronym address b big blockquote br cite code dd del dfn div dl dt em h1 h2 h3 h4 h5 h6 hr i ins kbd li ol
  p pre samp small span strong sub sup time tt ul var ] + [ "action-text-attachment", "figure", "figcaption" ]
CAMPFIRE_ATTRS = %w[ abbr alt cite class datetime height href lang name src title width xml:lang ] +
  %w[ sgid content-type url href filename filesize width height previewable presentation caption content ] + %w[ class ]

check.("a plain trix paragraph",
       V.sanitize_allowing("<div>hello world</div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div>hello world</div>")
check.("a line break",
       V.sanitize_allowing("<div>a<br>b</div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div>a<br>b</div>")
check.("inline formatting",
       V.sanitize_allowing("<div><strong>bold</strong> and <em>it</em></div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div><strong>bold</strong> and <em>it</em></div>")
check.("a link",
       V.sanitize_allowing("<div><a href=\"https://example.com\">link</a></div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div><a href=\"https://example.com\">link</a></div>")
check.("a list",
       V.sanitize_allowing("<ul><li>one</li><li>two</li></ul>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<ul><li>one</li><li>two</li></ul>")
check.("a blockquote",
       V.sanitize_allowing("<blockquote>quoted</blockquote>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<blockquote>quoted</blockquote>")
check.("preformatted text",
       V.sanitize_allowing("<pre>code here</pre>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<pre>code here</pre>")
check.("a heading",
       V.sanitize_allowing("<h1>heading</h1>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<h1>heading</h1>")
check.("multibyte text",
       V.sanitize_allowing("<div>emoji 👍 and text</div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div>emoji 👍 and text</div>")
check.("disallowed attributes go, class stays",
       V.sanitize_allowing("<div class=\"a b\" id=\"x\" data-foo=\"y\">t</div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div class=\"a b\">t</div>")
check.("target and rel are not in the list",
       V.sanitize_allowing("<a href=\"/rel/path\" target=\"_blank\" rel=\"noopener\">r</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"/rel/path\">r</a>")
check.("names downcase, values keep their case",
       V.sanitize_allowing("<a HREF=\"HTTPS://X.CO\" CLASS=\"c\">up</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"HTTPS://X.CO\" class=\"c\">up</a>")
check.("single quotes become double",
       V.sanitize_allowing("<a href='single'>q</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"single\">q</a>")
check.("an unquoted value is quoted",
       V.sanitize_allowing("<a href=bare>u</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"bare\">u</a>")
check.("an empty href survives",
       V.sanitize_allowing("<a href=\"\">empty</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"\">empty</a>")
check.("a bare ampersand in a value is escaped",
       V.sanitize_allowing("<span title=\"a&b\">bare amp</span>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<span title=\"a&amp;b\">bare amp</span>")
check.("a well-formed reference in a value is kept",
       V.sanitize_allowing("<span title=\"a&amp;b\">amp</span>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<span title=\"a&amp;b\">amp</span>")
check.("quotes inside a single-quoted value",
       V.sanitize_allowing("<span title='has \"quotes\"'>q2</span>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<span title=\"has &quot;quotes&quot;\">q2</span>")
check.("javascript: is dropped",
       V.sanitize_allowing("<a href=\"javascript:alert(1)\">j</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j</a>")
check.("mixed-case javascript: is dropped",
       V.sanitize_allowing("<a href=\"JaVaScRiPt:alert(1)\">j2</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j2</a>")
check.("a tab inside the scheme does not hide it",
       V.sanitize_allowing("<a href=\"java\tscript:alert(1)\">j3</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j3</a>")
check.("&colon; does not hide it",
       V.sanitize_allowing("<a href=\"javascript&colon;alert(1)\">j4</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j4</a>")
check.("a semicolonless numeric colon does not hide it",
       V.sanitize_allowing("<a href=\"javascript&#58alert(1)\">j5</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j5</a>")
check.("a hex numeric colon does not hide it",
       V.sanitize_allowing("<a href=\"javascript&#x3a;alert(1)\">j6</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j6</a>")
check.("a leading space does not hide it",
       V.sanitize_allowing("<a href=\" javascript:alert(1)\">j7</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j7</a>")
check.("an encoded scheme letter does not hide it",
       V.sanitize_allowing("<a href=\"&#106;avascript:x\">j9</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j9</a>")
check.("mailto passes",
       V.sanitize_allowing("<a href=\"mailto:x@y.z\">m</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"mailto:x@y.z\">m</a>")
check.("data text/html is dropped",
       V.sanitize_allowing("<a href=\"data:text/html,x\">d1</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>d1</a>")
check.("data image/png passes",
       V.sanitize_allowing("<a href=\"data:image/png;base64,AAAA\">d2</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"data:image/png;base64,AAAA\">d2</a>")
check.("tel passes",
       V.sanitize_allowing("<a href=\"tel:+1234\">t</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"tel:+1234\">t</a>")
check.("a protocol-relative url passes",
       V.sanitize_allowing("<a href=\"//proto.rel/x\">pr</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"//proto.rel/x\">pr</a>")
check.("a fragment passes",
       V.sanitize_allowing("<a href=\"#anchor\">an</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"#anchor\">an</a>")
check.("script strips to its text",
       V.sanitize_allowing("<script>alert(1)</script>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "alert(1)")
check.("markup inside script is character data",
       V.sanitize_allowing("<script><b>hi</b></script>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "&lt;b&gt;hi&lt;/b&gt;")
check.("a close tag with a space still closes rawtext",
       V.sanitize_allowing("<script >x</script >y", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "xy")
check.("style strips to its text",
       V.sanitize_allowing("<style>.x{color:red}</style>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       ".x{color:red}")
check.("form and input strip to their children",
       V.sanitize_allowing("<form action=\"/x\"><input name=\"a\"><button>go</button></form>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "go")
check.("iframe content is character data",
       V.sanitize_allowing("<iframe><b>x</b></iframe>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "&lt;b&gt;x&lt;/b&gt;")
check.("noscript children are markup",
       V.sanitize_allowing("<noscript><b>x</b></noscript>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<b>x</b>")
check.("textarea content is character data",
       V.sanitize_allowing("<textarea><b>x</b></textarea>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "&lt;b&gt;x&lt;/b&gt;")
check.("plaintext eats the rest",
       V.sanitize_allowing("<plaintext><b>x</b>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "&lt;b&gt;x&lt;/b&gt;")
check.("a table strips to its text",
       V.sanitize_allowing("<table><tr><td>cell</td></tr></table>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "cell")
check.("svg is pruned whole",
       V.sanitize_allowing("<svg><script>alert(1)</script></svg>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "")
check.("a self-closed svg prunes nothing after it",
       V.sanitize_allowing("<svg/><b>after</b>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<b>after</b>")
check.("nested foreign content is pruned whole",
       V.sanitize_allowing("<svg><foreignObject><b>x</b></foreignObject></svg>tail", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "tail")
check.("math is pruned whole",
       V.sanitize_allowing("<math><mtext><script>x</script></mtext></math>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "")
check.("an event handler is dropped",
       V.sanitize_allowing("<div onclick=\"alert(1)\">c</div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div>c</div>")
check.("an unclosed tag is closed at the end",
       V.sanitize_allowing("<b>unclosed", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<b>unclosed</b>")
check.("a stray close is dropped",
       V.sanitize_allowing("</b>stray close", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "stray close")
check.("upper-case names normalize",
       V.sanitize_allowing("<B CLASS=y>x</B>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<b class=\"y\">x</b>")
check.("two unclosed tags close innermost-first",
       V.sanitize_allowing("<div><span>nested unclosed</div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div><span>nested unclosed</span></div>")
check.("same-name nesting is kept as written",
       V.sanitize_allowing("<b>one<b>two</b>three</b>four", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<b>one<b>two</b>three</b>four")
check.("a stray < is escaped",
       V.sanitize_allowing("< notatag >", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "&lt; notatag &gt;")
check.("an empty tag is text",
       V.sanitize_allowing("<>empty<>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "&lt;&gt;empty&lt;&gt;")
check.("an unterminated tag swallows the rest",
       V.sanitize_allowing("<div", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "")
check.("text around tags",
       V.sanitize_allowing("text < then <b>tag</b>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "text &lt; then <b>tag</b>")
check.("a bare > is escaped",
       V.sanitize_allowing("a > b", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "a &gt; b")
check.("a comment vanishes",
       V.sanitize_allowing("before<!-- comment -->after", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "beforeafter")
check.("a commented script vanishes with it",
       V.sanitize_allowing("<!-- <script>alert(1)</script> -->x", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "x")
check.("a doctype vanishes",
       V.sanitize_allowing("<!DOCTYPE html><p>doc</p>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<p>doc</p>")
check.("a processing instruction vanishes",
       V.sanitize_allowing("<?php echo 1 ?>y", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "y")
check.("cdata vanishes",
       V.sanitize_allowing("<![CDATA[cdata]]>z", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "z")
check.("a well-formed reference is kept",
       V.sanitize_allowing("a &amp; b", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "a &amp; b")
check.("a bare ampersand is escaped",
       V.sanitize_allowing("a & b", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "a &amp; b")
check.("escaped angle brackets stay escaped",
       V.sanitize_allowing("&lt;tag&gt;", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "&lt;tag&gt;")
# DIVERGES, by the header's declared policy — the gem answers "café <b>bold</b>".
check.("a named reference stays as written",
       V.sanitize_allowing("caf&eacute; <b>bold</b>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "caf&eacute; <b>bold</b>")
# DIVERGES, by the header's declared policy — the gem answers "¬anentity;".
check.("a malformed reference is escaped",
       V.sanitize_allowing("&notanentity;", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "&notanentity;")
check.("an action-text-attachment round-trips",
       V.sanitize_allowing("<action-text-attachment sgid=\"x\" content-type=\"image/png\" url=\"http://x/y.png\" caption=\"c\"></action-text-attachment>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<action-text-attachment sgid=\"x\" content-type=\"image/png\" url=\"http://x/y.png\" caption=\"c\"></action-text-attachment>")
check.("deep nesting round-trips",
       V.sanitize_allowing("<div><div><div>deep</div></div></div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div><div><div>deep</div></div></div>")
check.("figure and figcaption round-trip",
       V.sanitize_allowing("<figure><figcaption>cap</figcaption></figure>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<figure><figcaption>cap</figcaption></figure>")
check.("angle brackets inside a value stay raw",
       V.sanitize_allowing("<span title=\"a<b>c\">lt</span>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<span title=\"a<b>c\">lt</span>")
check.("newlines between attributes normalize",
       V.sanitize_allowing("<a\nhref=\"http://x\"\nclass=\"y\">nl</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"http://x\" class=\"y\">nl</a>")
check.("the first duplicate attribute wins",
       V.sanitize_allowing("<a href=\"http://a\" href=\"javascript:x\">dup</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"http://a\">dup</a>")
check.("a style attribute is not in the list",
       V.sanitize_allowing("<div style=\"color:red\">styled</div>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<div>styled</div>")
check.("a void element self-closing slash is dropped",
       V.sanitize_allowing("<br/>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<br>")
check.("tagless input is unchanged",
       V.sanitize_allowing("hello", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "hello")
check.("empty input is empty",
       V.sanitize_allowing("", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "")
# DIVERGES, by the header's declared policy — the gem answers "<b>a</b><p><b>b</b>c</p>".
check.("mis-nested inline tags",
       V.sanitize_allowing("<b>a<p>b</b>c", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<b>a<p>b</p></b>c")
# DIVERGES, by the header's declared policy — the gem answers "<p>a</p><div>b</div>c<p></p>".
check.("a p closed by a block child",
       V.sanitize_allowing("<p>a<div>b</div>c</p>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<p>a<div>b</div>c</p>")
# DIVERGES, by the header's declared policy — the gem answers "<a href=\"jav…ascript:alert(1)\">j8</a>".
check.("a C1 numeric reference in a url",
       V.sanitize_allowing("<a href=\"jav&#x85;ascript:alert(1)\">j8</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a>j8</a>")
# DIVERGES, by the header's declared policy — the gem answers "<a href=\"http://x\">enc</a>".
check.("an entity-encoded colon in a kept url",
       V.sanitize_allowing("<a href=\"http&#58;//x\">enc</a>", CAMPFIRE_TAGS, CAMPFIRE_ATTRS),
       "<a href=\"http&#58;//x\">enc</a>")

# ── the default tables and the refusals ───────────────────────────────

check.("the one-argument form uses the gem's default tables",
       V.sanitize(%q{<img src="x.png" onerror="alert(1)">}),
       %q{<img src="x.png">})
check.("tagless input is served unchanged through the default form",
       V.sanitize("plain title"), "plain title")

refused = lambda do |label, &blk|
  begin
    blk.call
    fail_count += 1
    puts "FAIL #{label}: did not raise"
  rescue NotImplementedError
    puts "ok   #{label}"
  end
end
refused.("an allow-list naming script is refused") do
  V.sanitize_allowing("<p>x</p>", ["p", "script"], ["class"])
end
refused.("an allow-list naming svg is refused") do
  V.sanitize_allowing("<p>x</p>", ["p", "svg"], ["class"])
end
refused.("allowing the style attribute is refused") do
  V.sanitize_allowing("<p>x</p>", ["p"], ["class", "style"])
end

if fail_count == 0
  puts "ALL OK"
else
  puts "#{fail_count} FAILED"
  exit 1
end
