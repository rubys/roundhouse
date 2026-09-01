# scripts/campfire-compare-diff.rb — normalize + diff the artifacts two
# campfire lanes dumped (see scripts/campfire-compare for the frame).
#
# Two ranks of rewrite, and the difference is the contract:
#
#   * VOLATILE — bytes no two runs agree on even within one lane
#     (timestamps, CSRF tokens, signed blobs, asset digests). Masked
#     silently.
#   * KNOWN DIVERGENCE — the lanes genuinely disagree and the gap is
#     written down in docs/pipeline/runtime.md. Each rewrite names its
#     entry and is PRINTED when it fires, so the run says what it
#     forgave; retire the rewrite when the entry closes.
#
# Every artifact GATES (non-zero exit on a residual diff): the two
# broadcast frames since 2026-08-31, and the room page since 2026-09-01
# — its triage closed with five emit fixes (nil-attr rule, boolean-attr
# spelling, STI polymorphic routes, Rails' trix-input spelling, the
# gem-shipped trix.css) plus one ledgered forgiveness (the
# fragment-cache CSRF entry above). `:report` remains the mode a NEW
# artifact starts in until its findings are triaged.
#
# Usage: campfire-compare-diff.rb RAILS_DIR EMIT_DIR

rails_dir, emit_dir = ARGV
abort "usage: campfire-compare-diff.rb RAILS_DIR EMIT_DIR" unless rails_dir && emit_dir

VOLATILE = [
  [/datetime="[^"]*"/, 'datetime="TS"'],
  [/data-message-timestamp="\d+"/, 'data-message-timestamp="TS"'],
  [/data-message-updated-at="\d+"/, 'data-message-updated-at="TS"'],
  [/data-sort-value="\d+"/, 'data-sort-value="TS"'],
  [/name="authenticity_token" value="[^"]*"/, 'name="authenticity_token" value="CSRF"'],
  [/name="csrf-token" content="[^"]*"/, 'name="csrf-token" content="CSRF"'],
  # Signed blobs: avatar sgids and stream signatures verify against each
  # lane's own secret; the CONTENT equivalence claim is about everything
  # around them.
  [%r{/users/[A-Za-z0-9%_=-]{20,}(--[0-9a-f]+)?/avatar}, "/users/SGID/avatar"],
  [/signed-stream-name="[^"]*"/, 'signed-stream-name="SIGNED"'],
  [/\?v=\d+/, "?v=V"],
  # Rails production serves digested assets (propshaft); the emit serves
  # plain paths. Same logical asset, different spelling. `@` because
  # importmap pins scoped packages (`@rails--request-3fc8489d.js`).
  [/(\/assets\/[@A-Za-z0-9_\/.-]+?)-[0-9a-f]{8,}\.(\w+)/, '\1.\2'],
  # Each lane serves on its own port, and absolute self-URLs (the
  # copy-link button) embed it.
  [%r{http://127\.0\.0\.1(:\d+)?/}, "http://HOST/"],
]

KNOWN = [
  # Rails' message-fragment cache serves whatever render FIRST warmed it:
  # a page GET (session → boost forms carry the authenticity_token input)
  # or a broadcast (session-less → they don't). So token presence in the
  # room page's boost forms encodes each row's RENDER HISTORY, not the
  # app — Rails' own page mixes both spellings after a cable post. The
  # emit re-renders per request (it has no fragment cache) and always
  # carries the token. Both submit fine: the JS posts X-CSRF-Token from
  # the page meta. Scoped to the room page so the FRAME gate keeps
  # failing on a token that reappears in a broadcast.
  # See "Room-page boost forms and the fragment cache" in runtime.md.
  { name: "room-page boost forms and the fragment cache",
    only: "room.html",
    apply: ->(s) { s.gsub(%r{(<form[^>]*/boosts"[^>]*>)\s*<input[^>]*name="authenticity_token"[^>]*>}, '\1') } },
  # (Retired 2026-08-31: "dom_id and STI" — the base model's dom_prefix
  # dispatches on the type column now, so an STI row answers the
  # subclass's dom class on every lane, exactly as Rails does. A
  # difference here fails the run again.)
  # (Retired 2026-08-31: "broadcast row identity" — dom_id now derives
  # its key from the model's own `to_key`, so every per-message id
  # matches Rails byte for byte, client_message_id and all. A
  # difference here fails the run again, which is the point of
  # retiring the mask with the fix.)
  # (Retired 2026-08-31: "broadcast forms and CSRF" — the lowered
  # broadcast html renders inside ViewHelpers.broadcast_render, whose
  # flag makes csrf_token_hidden_input answer "", matching Rails'
  # session-less broadcast renderer. A token input in a frame fails the
  # run again.)
]

# ── DOM-shape canonicalization ─────────────────────────────────────────
#
# "Compare = DOM, not bytes": the lanes spell the same element
# differently — Rails' link_to writes `href` last where the emit writes
# it first, and ERB's tag helpers close void elements ` />` where the
# emit writes `>`. A browser parses both to the same node, so the diff
# must too: every start tag is re-serialized with its attributes sorted
# and the void slash dropped. Quote-aware, because attribute values
# legally carry `<` and `>`.
def canonicalize_tags(s)
  out = +""
  i = 0
  n = s.length
  while i < n
    c = s[i]
    if c == "<" && s[i + 1] =~ /[a-zA-Z]/
      j = i + 1
      quote = nil
      while j < n
        cj = s[j]
        if quote
          quote = nil if cj == quote
        elsif cj == '"' || cj == "'"
          quote = cj
        elsif cj == ">"
          break
        end
        j += 1
      end
      raw = s[(i + 1)...j]
      out << canonical_tag(raw)
      i = j + 1
    else
      out << c
      i += 1
    end
  end
  out
end

def canonical_tag(raw)
  raw = raw.sub(%r{\s*/\z}, "")
  name = raw[/\A[^\s]+/]
  rest = raw[name.length..] || ""
  attrs = []
  i = 0
  n = rest.length
  while i < n
    i += 1 while i < n && rest[i] =~ /\s/
    break if i >= n
    astart = i
    i += 1 while i < n && rest[i] !~ /[\s=]/
    aname = rest[astart...i]
    i += 1 while i < n && rest[i] =~ /\s/
    if rest[i] == "="
      i += 1
      i += 1 while i < n && rest[i] =~ /\s/
      q = rest[i]
      if q == '"' || q == "'"
        i += 1
        vstart = i
        i += 1 while i < n && rest[i] != q
        value = rest[vstart...i]
        i += 1 if i < n
      else
        vstart = i
        i += 1 while i < n && rest[i] !~ /\s/
        value = rest[vstart...i]
      end
      attrs << [aname, value]
    else
      attrs << [aname, nil]
    end
  end
  serialized = attrs.sort_by(&:first).map { |k, v| v.nil? ? k : %(#{k}="#{canonical_value(v)}") }.join(" ")
  serialized.empty? ? "<#{name}>" : "<#{name} #{serialized}>"
end

# A browser reads `data-action="click-&gt;popup#close"` and
# `data-action="click->popup#close"` as the SAME attribute value — ERB's
# tag helpers escape `>` where hand-written template HTML doesn't, and
# the two lanes disagree in BOTH directions. Decode the named entities
# (one pass, so `&amp;gt;` stays the text `&gt;`), then re-encode only
# what a double-quoted serialization requires, so every value has ONE
# canonical spelling.
ENTITIES = { "&gt;" => ">", "&lt;" => "<", "&quot;" => '"', "&#39;" => "'", "&apos;" => "'", "&amp;" => "&" }.freeze
def canonical_value(v)
  v.gsub(/&(?:gt|lt|quot|#39|apos|amp);/) { |m| ENTITIES[m] }
   .gsub("&", "&amp;").gsub('"', "&quot;")
end

def normalize(text, artifact_name, fired)
  out = text.dup
  VOLATILE.each { |(re, rep)| out.gsub!(re, rep) }
  KNOWN.each do |k|
    next if k[:only] && k[:only] != artifact_name
    before = out
    out = k[:apply].call(out)
    fired << k[:name] if out != before
  end
  canonicalize_tags(out)
end

def diff_head(a_path, b_path, lines)
  `diff -u #{a_path} #{b_path} 2>/dev/null`.lines.first(lines).join
end

# The EQUALITY form: inter-tag and line-break whitespace collapsed the
# way a browser collapses it — runs become one space, and the space
# before a closing tag or between two tags goes entirely. The
# `.normalized` files keep their line structure so the human diff stays
# readable; this tighter form is only what == runs on. (Imprecise
# inside <pre>, where whitespace is real — no gated artifact carries a
# <pre> today; the day a seeded code-block message adds one, this
# collapse needs a pre-guard.)
def tighten(s)
  s.gsub(/\s+/, " ").gsub(%r{ +</}, "</").gsub(/> +</, "><")
end

failed = false
report = []

{ "frame_text.html" => :gate, "frame_html.html" => :gate, "room.html" => :gate }.each do |name, mode|
  a = File.join(rails_dir, name)
  b = File.join(emit_dir, name)
  unless File.exist?(a) && File.exist?(b)
    puts "  \e[31mFAIL\e[0m #{name}: missing artifact (#{File.exist?(a) ? "emit" : "rails"} side)"
    failed = true
    next
  end
  fired = []
  na = normalize(File.read(a), name, fired)
  nb = normalize(File.read(b), name, fired)
  File.write(a + ".normalized", na)
  File.write(b + ".normalized", nb)
  if tighten(na) == tighten(nb)
    puts "  \e[32mok\e[0m   #{name} — equivalent#{fired.empty? ? "" : " (#{fired.uniq.length} known divergence(s) forgiven)"}"
    fired.uniq.each { |f| puts "       forgiven: #{f}" }
  else
    d = diff_head(a + ".normalized", b + ".normalized", 30)
    if mode == :gate
      puts "  \e[31mFAIL\e[0m #{name}: lanes disagree beyond the known divergences"
      failed = true
    else
      # Count the diff's own lines, not a positional zip — one inserted
      # line early in the file must not count everything after it.
      diffs = `diff #{a}.normalized #{b}.normalized 2>/dev/null`.lines.count { |l| l.start_with?("< ", "> ") }
      puts "  \e[33mreport\e[0m #{name}: #{diffs} differing line(s) — reported, not gated"
    end
    fired.uniq.each { |f| puts "       forgiven: #{f}" }
    report << "--- #{name} (rails vs emit, normalized) ---\n#{d}"
  end
end

puts
report.each { |r| puts r; puts }
if failed
  puts "\e[1;31mcampfire compare failed\e[0m — an artifact diverged beyond the ledger"
  exit 1
end
puts "\e[1;32mcampfire compare complete\e[0m — the room page and the frames the two lanes serve are equivalent"
