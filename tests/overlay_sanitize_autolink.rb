# Driver for tests/overlay_sanitize_autolink.rs — loads the CRuby
# overlay's escape surface WITHOUT an emitted tree and exercises it.
#
# The overlay is where `sanitize` / `strip_tags` / `auto_link` actually
# live, and nothing else covers them: campfire's suite passes with the
# gem hidden (no test asserts message-body markup), and the only other
# check is the oracle comparison, which does not run in CI.
$stdout.sync = true
require "cgi"
require "json"
ROOT = ARGV[0]
require File.join(ROOT, "runtime/spinel/scaffold/ruby_overlay/runtime/active_support_core_ext")
require File.join(ROOT, "runtime/ruby/action_view/view_helpers")
require File.join(ROOT, "runtime/ruby/action_view/view_helpers_ext")
# BEFORE the safe buffer, exactly as boot.rb orders them (action_text at
# line 66, the overlay at 106). The overlay REOPENS `ActionText::Content`
# to give `to_s` the content layout; loaded the other way round it would
# DEFINE an empty class and the real one would then clobber the override
# — silently, since `to_s` would still answer, just without the wrapper.
require File.join(ROOT, "runtime/ruby/action_text")
require File.join(ROOT, "runtime/spinel/scaffold/ruby_overlay/runtime/action_view_safe_buffer")
require File.join(ROOT, "runtime/spinel/scaffold/ruby_overlay/runtime/action_view_sanitize")

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

puts "vendor #{RH_SANITIZER_VENDOR.inspect}"
# FAIL LOUDLY rather than skip. Without the gem the overlay falls back
# to the shared scanner, which refuses markup by design — so a "skip"
# here would silently stop covering the thing this file exists to cover.
if RH_SANITIZER_VENDOR.nil?
  puts "FAIL no sanitizer vendor — install rails-html-sanitizer " \
       "(CI does this in the `unit` job)"
  exit 1
end

# GOLDEN VALUES, every one measured against the real gem / ActionView.
# These run whether or not a reference gem is installed, which is what
# makes this a gate rather than a probe.
check.("sanitize keeps an allow-listed tag",
       V.sanitize("Deploy is green — <b>3.4.10</b>.").to_s,
       "Deploy is green — <b>3.4.10</b>.")
check.("sanitize drops a script tag but KEEPS its text",
       V.sanitize("<script>alert(1)</script>after").to_s, "alert(1)after")
check.("sanitize drops a javascript: href, keeps the anchor",
       V.sanitize(%q{<a href="javascript:evil()">j</a>}).to_s, "<a>j</a>")
check.("sanitize escapes a bare angle bracket",
       V.sanitize("a < b and 5 > 4").to_s, "a &lt; b and 5 &gt; 4")
check.("strip_tags keeps the text of a dropped element",
       V.strip_tags("<b>Hi</b> &amp; <script>bad()</script>there").to_s,
       "Hi &amp; bad()there")
check.("strip_tags escapes a `<` that opens nothing",
       V.strip_tags("a < b and c > d").to_s, "a &lt; b and c &gt; d")
check.("auto_link wraps a url and keeps the html option",
       V.auto_link("see https://example.com/a", html: { target: "_blank" }).to_s,
       %q{see <a target="_blank" href="https://example.com/a">https://example.com/a</a>})
check.("auto_link leaves an already-linked url alone",
       V.auto_link(%q{<a href="https://x.com">https://x.com</a>}).to_s,
       %q{<a href="https://x.com">https://x.com</a>})
check.("auto_link does not swallow trailing punctuation",
       V.auto_link("go https://example.com/a.").to_s,
       %q{go <a href="https://example.com/a">https://example.com/a</a>.})
check.("auto_link keeps a bracket the url opened",
       V.auto_link("see https://en.wikipedia.org/wiki/Foo_(bar) end").to_s,
       %q{see <a href="https://en.wikipedia.org/wiki/Foo_(bar)">https://en.wikipedia.org/wiki/Foo_(bar)</a> end})
check.("auto_link on empty text is empty", V.auto_link("").to_s, "")

# `h` is an ALIAS: an html_safe value passes through unescaped, and a
# plain String is escaped. Get this backwards and every formatted
# message renders its own tags as visible text.
check.("h escapes a plain String", V.h("<b>x</b>"), "&lt;b&gt;x&lt;/b&gt;")
check.("h passes an html_safe value through", V.h(SafeString.new("<b>x</b>")), "<b>x</b>")

# The whole campfire expression, end to end.
content = ActionText::Content.new("Deploy <b>3.4.10</b> — https://ex.co/a")
check.("the message-body chain renders markup AND links",
       V.auto_link(V.h(content), html: { target: "_blank" }).to_s,
       %q{Deploy <b>3.4.10</b> — <a target="_blank" href="https://ex.co/a">https://ex.co/a</a>})

puts(fail_count.zero? ? "ALL OK" : "#{fail_count} FAILED")
exit(fail_count.zero? ? 0 : 1)
