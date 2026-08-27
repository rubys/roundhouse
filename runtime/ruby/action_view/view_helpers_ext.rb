# Ruby-family-only ViewHelpers extensions — a reopen, same pattern as
# active_record/connection.rb: NOT listed in runtime_loader's strict-
# target tables (the scaffold dir-walk ships it to spinel/CRuby/JRuby
# only). These bodies exercise emitter surface the elixir/rust lanes
# don't carry yet (String#include? renders .__struct__ on elixir;
# while-loops with post-loop reads hit the functionalize sign-threading
# gap) — they join the universal file when those lanes' emitters catch
# up and lobsters reaches them.
module ActionView
  module ViewHelpers
    # `content_security_policy_nonce` — the per-request CSP script
    # nonce Rails interpolates into `<script nonce=…>`. The CSP HEADER
    # pipeline isn't modeled (no Content-Security-Policy response
    # header is emitted), so the nonce is inert to browsers; a stable
    # token keeps the layout's interpolation rendering without pulling
    # a randomness primitive into every target runtime.
    def self.content_security_policy_nonce
      "roundhouse-nonce"
    end

    # Rails `class_names` (alias of `token_list`): strings/arrays add
    # their tokens, hash entries contribute their key when the value is
    # truthy (`class_names("nav", current_page: cur == path)`), nil and
    # blank tokens drop. Joined with single spaces.
    def self.class_names(*args)
      tokens = []
      args.each do |arg|
        if arg.is_a?(Hash)
          arg.each { |k, v| tokens << k.to_s if v }
        elsif arg.is_a?(Array)
          arg.each do |a|
            s = a.to_s
            tokens << s unless s.strip.empty?
          end
        elsif !arg.nil?
          s = arg.to_s
          tokens << s unless s.strip.empty?
        end
      end
      tokens.join(" ")
    end

    # `number_with_delimiter(12345)` → "12,345" — comma grouping every
    # three digits, sign-aware. Integer-only, matching the signature
    # (every corpus arg is a count); while-loop over the digit string
    # so every target runtime types it; byte-equal to the CRuby overlay
    # variant it supersedes on the replay-locked /u page. The overlay's
    # `delimiter:` kwarg and float handling have no caller — the shared
    # version stays monomorphic.
    def self.number_with_delimiter(value)
      int = value.to_s
      sign = +""
      if int.start_with?("-")
        sign = "-"
        int = int[1, int.length - 1].to_s
      end
      out = +""
      i = int.length
      while i > 3
        out = "," + int[i - 3, 3].to_s + out
        i = i - 3
      end
      out = int[0, i].to_s + out
      sign + out
    end

    # Rails' sanitize_to_id — the default `id` a `*_tag` control
    # derives from its `name` ("tags[foo]" → "tags_foo"): drop "]",
    # replace every char outside [-a-zA-Z0-9:.] with "_". Char loop
    # over an inline membership literal (a module-const receiver reads
    # as an unresolved class in the strict typer), not gsub-with-regex:
    # portable and typed on every target runtime. The output alphabet
    # is attr-safe by construction, so call sites splice it unescaped.
    def self.sanitize_to_id(name)
      out = +""
      i = 0
      n = name.length
      while i < n
        # Two-arg slice + plain concat, the shapes every strict emitter
        # already ships (truncate's s[0, cutoff]); `out << c` renders as
        # an immutable-local .add() on Kotlin/Swift, and one-arg s[i]
        # isn't in the proven surface.
        c = name[i, 1].to_s
        if c != "]"
          out = out + ("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-:.".include?(c) ? c : "_")
        end
        i = i + 1
      end
      out
    end

    # `hidden_field_tag "boost[content]", "\u{1F389}"` →
    # `<input type="hidden" name="boost[content]" id="boost_content" value="\u{1F389}" />`
    #
    # The bare (builder-less) hidden field — campfire carries a quick
    # boost's emoji in one, eight per message. Sits here rather than in
    # the universal file because it derives its id with
    # `sanitize_to_id` above, which is ruby-family for the reasons that
    # file's header gives; it joins the universal one when
    # `sanitize_to_id` does.
    #
    # Three details measured against Rails 8.1 rather than guessed: the
    # `id` is the name run through `sanitize_to_id`; `value` is OMITTED
    # entirely when nil rather than rendered empty; and the tag closes
    # ` />`, the legacy `tag()` spelling the field helpers share —
    # `image_tag` beside it closes `>`, and they genuinely differ.
    #
    # Options ride `render_attrs`, which already drops a nil value and
    # expands a nested hash into `data-*`. That is what makes campfire's
    # two option spellings work without either being special-cased:
    # `data: { sessions_target: "x" }` becomes ` data-sessions-target="x"`,
    # and `id: nil` OVERWRITES the derived id with nil, which
    # `render_attrs` then drops — which is how Rails suppresses it too.
    # A rewritten key keeps its original insertion slot in Ruby, so the
    # attribute ORDER stays type/name/id/value whatever the caller
    # passes.
    def self.hidden_field_tag(name, value = nil, opts = {})
      name_s = name.to_s
      attrs = {}
      attrs[:type] = "hidden"
      attrs[:name] = name_s
      attrs[:id] = sanitize_to_id(name_s)
      attrs[:value] = value.to_s unless value.nil?
      opts.to_h.each do |k, v|
        attrs[k] = v
      end
      "<input#{render_attrs(attrs)} />"
    end

    # `number_with_precision(4.5678, precision: 2)` → "4.57" — the
    # overlay number-helper's exact shape; here so the spinel tree
    # carries it (users/show renders karma averages). On CRuby the
    # overlay's later require re-defines it, same bytes.
    def self.number_with_precision(value, precision: 3)
      format("%.#{precision}f", value.to_f)
    end

    # `number_to_human(5, format: "%n%u")` → "5"; `number_to_human(1500)`
    # → "1 Thousand". Rails scales by powers of 1000 with a unit label.
    # lobsters' `upvoter_score` passes a small INTEGER score + format
    # "%n%u", so the unit-less common case renders as the plain number;
    # integer-only (no float/precision machinery) keeps every site typed.
    def self.number_to_human(value, format: "%n %u")
      units = ["", "Thousand", "Million", "Billion", "Trillion", "Quadrillion"]
      neg = value < 0
      n = neg ? -value : value
      idx = 0
      while n >= 1000 && idx < units.length - 1
        n = n / 1000
        idx = idx + 1
      end
      num = neg ? "-" + n.to_s : n.to_s
      format.sub("%n", num).sub("%u", units[idx]).strip
    end

    # `time_ago_in_words` / `distance_of_time_in_words`. The bucket walk
    # is Rails' DateHelper verbatim (actionview/lib/action_view/helpers/
    # date_helper.rb) with the I18n lookups collapsed to actionview's
    # built-in :en strings — the corpus runs the default locale, and
    # per-route output parity against a real Rails lobsters needs the
    # exact wording ("about 1 hour", "less than a minute").
    #
    # These were a CRuby-only overlay until the spinel lane needed them
    # too; unified here rather than duplicated, so both trees render the
    # same bytes by construction (the CookieJar/ActionMailer pattern).
    # The overlay's stated reason for staying CRuby-only was that the
    # entry computation is `Time - Time`, which doesn't transpile
    # uniformly (Crystal yields Time::Span, the JVM wants
    # java.time.Duration) — but that argument lands on the strict
    # targets, and THIS FILE is already ruby-family-only (see the header:
    # it is deliberately absent from runtime_loader's tables). So the
    # boundary it describes is the boundary this file already sits on.
    MINUTES_IN_YEAR = 525600
    MINUTES_IN_QUARTER_YEAR = 131400
    MINUTES_IN_THREE_QUARTERS_YEAR = 394200

    def self.time_ago_in_words(from_time, include_seconds: false)
      distance_of_time_in_words(from_time, Time.now, include_seconds: include_seconds)
    end

    def self.distance_of_time_in_words(from_time, to_time, include_seconds: false)
      # Rails writes this as `from_time, to_time = to_time, from_time if
      # from_time > to_time`, reassigning its own parameters through a
      # multiple assignment. Bound to fresh locals instead: with the
      # params RBS-pinned to `::Time`, that swap emitted an `sp_RbVal`
      # into an `sp_Time` slot and the C stage refused it — but only
      # once the whole app tree was in the picture (the method compiles
      # standalone, and with a call site, and with none). Two ordinary
      # assignments are clearer than the swap anyway, and not
      # reassigning a parameter is the better habit regardless.
      earlier = from_time
      later = to_time
      if from_time > to_time
        earlier = to_time
        later = from_time
      end
      # `to_f` rather than Rails' bare `later - earlier`: `Time#-`
      # already RETURNS float seconds, so the arithmetic is identical,
      # but receiver-only dispatch can't tell a Duration argument (→
      # Time) from a Time one (→ Float) and so types `Time - x` as
      # untyped. Taking the epoch floats first keeps every expression
      # below concretely Float/Integer, which is what the framework
      # runtime's fully-typed invariant requires.
      elapsed = later.to_f - earlier.to_f
      distance_in_minutes = (elapsed / 60.0).round
      distance_in_seconds = elapsed.round

      case distance_in_minutes
      when 0..1
        unless include_seconds
          return distance_in_minutes == 0 ? "less than a minute" : "1 minute"
        end
        case distance_in_seconds
        when 0..4   then "less than 5 seconds"
        when 5..9   then "less than 10 seconds"
        when 10..19 then "less than 20 seconds"
        when 20..39 then "half a minute"
        when 40..59 then "less than a minute"
        else             "1 minute"
        end
      when 2...45       then "#{distance_in_minutes} minutes"
      when 45...90      then "about 1 hour"
      when 90...1440    then "about #{(distance_in_minutes.to_f / 60.0).round} hours"
      when 1440...2520  then "1 day"
      when 2520...43200 then "#{(distance_in_minutes.to_f / 1440.0).round} days"
      when 43200...86400
        months = (distance_in_minutes.to_f / 43200.0).round
        months == 1 ? "about 1 month" : "about #{months} months"
      when 86400...525600 then "#{(distance_in_minutes.to_f / 43200.0).round} months"
      else
        from_year = earlier.year
        from_year += 1 if earlier.month >= 3
        to_year = later.year
        to_year -= 1 if later.month < 3

        leap_years =
          if from_year > to_year
            0
          else
            fyear = from_year - 1
            (to_year / 4 - to_year / 100 + to_year / 400) -
              (fyear / 4 - fyear / 100 + fyear / 400)
          end
        minute_offset_for_leap_year = leap_years * 1440

        # Discount leap-year days so e.g. 80 years of minutes still reads
        # "about 80 years" (Rails' comment, same arithmetic).
        minutes_with_offset = distance_in_minutes - minute_offset_for_leap_year
        remainder = minutes_with_offset % MINUTES_IN_YEAR
        # Rails spells this `.div(...)`, which the Integer method table
        # doesn't carry; `/` on two Integers is the same floor division.
        distance_in_years = minutes_with_offset / MINUTES_IN_YEAR
        if remainder < MINUTES_IN_QUARTER_YEAR
          distance_in_years == 1 ? "about 1 year" : "about #{distance_in_years} years"
        elsif remainder < MINUTES_IN_THREE_QUARTERS_YEAR
          distance_in_years == 1 ? "over 1 year" : "over #{distance_in_years} years"
        else
          distance_in_years + 1 == 1 ? "almost 1 year" : "almost #{distance_in_years + 1} years"
        end
      end
    end

    # ── Sanitization ─────────────────────────────────────────────────

    # Rails' `strip_tags` — `Rails::HTML5::FullSanitizer#sanitize`,
    # which is an HTML5 PARSE followed by a text-content serialize, not
    # a `gsub(/<[^>]*>/, "")`. The difference is observable and was
    # measured against the real sanitizer, gem version 1.7.1:
    #
    #   "<b>Hi</b> &amp; <script>bad()</script>there"
    #                              -> "Hi &amp; bad()there"
    #   "a < b and c > d"          -> "a &lt; b and c &gt; d"
    #   "<b class=\"x>y\">z</b>"   -> "z"
    #   "unclosed <b tag"          -> "unclosed "
    #   "<>empty"                  -> "&lt;&gt;empty"
    #   "<!-- c -->visible"        -> "visible"
    #   "<!DOCTYPE html>x"         -> "x"
    #
    # So: a `<` that does not open a tag is TEXT and comes back
    # escaped; a `>` in a quoted attribute value does not end the tag;
    # an unterminated tag or comment swallows the rest of the input;
    # and the CONTENT of a dropped element survives (`bad()` above),
    # because only the tags are removed.
    #
    # DIVERGENCE, stated because it is invisible at the call site.
    # Rails DECODES entity references and re-serializes, which needs
    # HTML5's 2231-entry named-entity table. This pass instead leaves a
    # well-formed reference (`&name;`, `&#123;`, `&#xAB;`) exactly as
    # written and escapes a bare `&`. On everything that round-trips —
    # `&amp;`, `&lt;`, `&nbsp;` — the two agree byte for byte, and on
    # the rest (`&eacute;` here vs `é` there) they render identically
    # in a browser. They part company only on malformed input, where
    # HTML5's legacy no-semicolon matching applies: `&notanentity;` is
    # `¬anentity;` to Rails and unchanged here. Ledgered in
    # docs/pipeline/runtime.md.
    def self.strip_tags(html)
      s = html.to_s
      out = +""
      i = 0
      n = s.length
      while i < n
        c = s[i, 1].to_s
        if c == "<"
          if s[i, 4] == "<!--"
            close = s.index("-->", i + 4)
            # An unterminated comment eats the remainder, as in Rails.
            i = close.nil? ? n : close + 3
          elsif tag_open_at?(s, i)
            after = tag_end_index(s, i)
            i = after.nil? ? n : after
          else
            out = out + "&lt;"
            i = i + 1
          end
        elsif c == ">"
          out = out + "&gt;"
          i = i + 1
        elsif c == "&"
          len = entity_reference_length(s, i)
          if len == 0
            out = out + "&amp;"
            i = i + 1
          else
            out = out + s[i, len].to_s
            i = i + len
          end
        else
          out = out + c
          i = i + 1
        end
      end
      out
    end

    # Rails' `sanitize` — the SAFE-LIST sanitizer, which keeps an
    # allow-listed subset of markup (`b`, `a href`, `img src`, …) and
    # drops the rest while keeping its text.
    #
    # NOT IMPLEMENTED for input that contains markup, DELIBERATELY. The
    # allow-list is a rule table (42 tags, 13 attributes, a per-attribute
    # URL-protocol list, and CSS sanitizing behind `style`), and the two
    # ways to fake it are both wrong in a way nobody would see: dropping
    # every tag silently discards markup the caller asked to keep, and
    # keeping every tag is a cross-site-scripting hole. A helper named
    # `sanitize` is the last place to guess.
    #
    # What IS implemented is the case the corpus actually has. campfire's
    # `Opengraph::Metadata#sanitize_fields` writes
    # `sanitize(strip_tags(title))` — by construction there is no markup
    # left by the time `sanitize` sees it, and on tagless input Rails'
    # safe-list sanitizer is the identity (measured, same gem version).
    # So that call site is exactly right, and any other one raises with
    # its own name in the message instead of returning something
    # plausible.
    def self.sanitize(html)
      s = html.to_s
      if s.include?("<")
        raise NotImplementedError,
              "ActionView::ViewHelpers.sanitize: the safe-list sanitizer is not " \
              "modelled; only tagless input (what `sanitize(strip_tags(x))` " \
              "produces) is served — see runtime/ruby/action_view/view_helpers_ext.rb"
      end
      s
    end

    # Does a tag start at `i` (where `s[i]` is "<")? HTML5 opens an
    # element on `<` + letter and closes one on `</` + letter; anything
    # else — `<3`, `< b`, `<>` — is text. `<!` and `<?` are the bogus
    # comment / markup declaration forms (`<!DOCTYPE html>`, `<?php ?>`),
    # which end at the next `>` and vanish like a tag.
    def self.tag_open_at?(s, i)
      c = s[i + 1, 1].to_s
      return true if c == "!" || c == "?"
      c = s[i + 2, 1].to_s if c == "/"
      # Inline literal, not a module const: a module-const receiver
      # reads as an unresolved class in the strict typer (same reason
      # `sanitize_to_id` above spells its alphabet out).
      "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".include?(c) && c != ""
    end

    # Index just past the `>` that closes the tag opening at `i`, or nil
    # if the input ends first. A `>` inside a quoted attribute value does
    # not close the tag: `<b class="x>y">z</b>` strips to "z", not to
    # "y\">z".
    def self.tag_end_index(s, i)
      j = i + 1
      n = s.length
      quote = ""
      out = nil
      while j < n && out.nil?
        c = s[j, 1].to_s
        if quote != ""
          quote = "" if c == quote
        elsif c == "\"" || c == "'"
          quote = c
        elsif c == ">"
          out = j + 1
        end
        j = j + 1
      end
      out
    end

    # Length of the entity reference starting at `i` (where `s[i]` is
    # "&"), or 0 if what follows is not a well-formed one. Three forms:
    # `&name;`, `&#123;`, `&#xAB;` — the semicolon is required here,
    # which is where this parts company with HTML5's legacy matching
    # (see the note on `strip_tags`).
    def self.entity_reference_length(s, i)
      n = s.length
      j = i + 1
      digits = ""
      if s[j, 1].to_s == "#"
        j = j + 1
        if s[j, 1].to_s == "x" || s[j, 1].to_s == "X"
          j = j + 1
          digits = "0123456789abcdefABCDEF"
        else
          digits = "0123456789"
        end
      else
        digits = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ" + "0123456789"
        # A named reference must OPEN with a letter: `&1;` is not one.
        return 0 unless "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".include?(s[j, 1].to_s) && s[j, 1].to_s != ""
      end
      start = j
      while j < n && digits.include?(s[j, 1].to_s) && s[j, 1].to_s != ""
        j = j + 1
      end
      return 0 if j == start
      return 0 unless s[j, 1].to_s == ";"
      j + 1 - i
    end

    # ── auto_link ────────────────────────────────────────────────────

    # Rails' `auto_link` — the `rails_autolink` gem, PORTED as a scanner
    # so every target compiles it. campfire renders EVERY message body
    # through it (`MessagesHelper#message_presentation` is
    # `auto_link h(...), html: { target: "_blank" }`), so unlike
    # `sanitize` this is on the read path and a refusing stub is not an
    # option.
    #
    # The gem is TWO REGEXES AND A RULE TABLE, and the rule table is the
    # part that matters: which schemes count, which characters end a
    # URL, which trailing punctuation is the sentence's rather than the
    # link's, what an e-mail local part may contain. Ported, not derived
    # — deriving one of these from taste is how you link half a URL.
    # The gem's own source, for the record:
    #
    #   AUTO_LINK_RE = %r{
    #     (?: ((?:ed2k|ftp|http|https|irc|mailto|news|gopher|nntp|telnet|
    #            webcal|xmpp|callto|feed|svn|urn|aim|rsync|tag|ssh|sftp|
    #            rtsp|afs|file):)// | www\.\w )
    #     [^\s< "]+
    #   }ix
    #   AUTO_EMAIL_LOCAL_RE = /[\w.!#\$%&'*\/=?^`{|}~+-]/
    #   AUTO_EMAIL_RE = /(?<!#{LOCAL})[\w.!#\$%+-]\.?#{LOCAL}*@
    #                    [\w-]+(?:\.[\w-]+)+/
    #
    # WHY A SCANNER AND NOT THOSE REGEXES. Two reasons, and only the
    # second is spinel's:
    #
    #   * `\p{Word}` in the trailing-punctuation strip and the lookbehind
    #     in the e-mail pattern are not a portable regex subset;
    #     matz/spinel#4143 rejects `\p{...}` at compile time outright.
    #   * The gem drives them with `gsub` + `$&` / `$'` / `` $` `` and
    #     decides from the text on either side of the match whether it is
    #     already inside a tag or inside an `<a>`. A left-to-right scan
    #     that carries that state is the same answer without the globals,
    #     which no strict target models.
    #
    # DIVERGENCE, and it is a SECURITY one, so it is stated here and
    # ledgered in docs/pipeline/runtime.md. Rails' `auto_link`
    # safe-list-sanitizes the whole body first. This does NOT: the
    # safe-list pass is HTML5 tree construction rather than filtering
    # (see the header of `ruby_overlay/runtime/action_view_sanitize.rb`
    # and the note on `sanitize` above), so the shared runtime refuses it
    # rather than approximate it, and refusing is not available on the
    # read path. The body therefore passes through as given.
    #
    # What that does and does not cost:
    #
    #   * The links this helper CREATES are still safe by the rule table
    #     above — the scheme list has no `javascript:` in it and the
    #     `www.` branch is prefixed `http://`, so `auto_link` cannot
    #     manufacture a scripting URL out of text.
    #   * What is lost is Rails' SECOND layer over markup that was
    #     already in the body. campfire's is ActionText content that
    #     arrived through `h`, so the first layer is the one doing the
    #     work — but an app that feeds `auto_link` raw user HTML and
    #     leans on this pass to clean it gets no cleaning here.
    #
    # The CRuby lane does better and does not use this: the overlay
    # serves `auto_link` from the real gem chain, which is why the two
    # are pinned against each other in `tests/overlay_sanitize_autolink.rb`.
    #
    # `sanitize:` is ACCEPTED AND HAS NO EFFECT, which is a stronger
    # statement than it looks and was measured rather than assumed. In
    # the gem the flag does two things: it runs the body pass (skipped
    # here), and it is forwarded as `content_tag`'s fourth argument,
    # `escape`. That second one never bites — `escape` is true only when
    # the sanitize ran, and a sanitized value is an html_safe buffer,
    # which `content_tag` splices raw regardless. All three settings
    # (unset, `false`, `true`) therefore produce the same anchor text in
    # the gem, and all three produce it here. Kept on the signature so a
    # call site that passes it still compiles.
    #
    # NOT MODELLED: the block form (`auto_link(text) { |url| ... }`,
    # which rewrites the link TEXT). No corpus call site passes one, and
    # a block argument through the strict targets is a shape this file
    # has no other use for. `sanitize_options:` likewise: it configures
    # a pass that does not run here.
    def self.auto_link(text, html: {}, link: :all, sanitize: false)
      s = text.to_s
      return "" if s.empty?
      do_urls = link != :email_addresses
      do_emails = link != :urls
      out = +""
      i = 0
      n = s.length
      # `auto_linked?` in the gem, carried forward instead of looked
      # back for. Both of its clauses are reproduced EXACTLY, and the
      # first one is cruder than "skip over tags" — which is the whole
      # reason it is spelled out here:
      #
      #   pre =~ /<[^>]+$/ && post =~ /^[^>]*>/
      #
      # is "the last `<` is unclosed, has at least ONE character after
      # it, and a `>` comes later". So the character immediately after a
      # `<` is NOT inside a tag as far as this helper is concerned, and
      # `addr <foo@bar.com>` gets its e-mail linked where a tag-skipping
      # scanner would swallow the lot. `open_lt` is that last unclosed
      # `<`; `open_lt_closes` is the `post` half, decided once when the
      # `<` is passed because no later position can change the answer.
      open_lt = -1
      open_lt_closes = false
      in_anchor = false
      while i < n
        c = s[i, 1].to_s
        if c == "<"
          open_lt = i
          open_lt_closes = !s.index(">", i).nil?
          out = out + c
          i = i + 1
        elsif c == ">" && open_lt >= 0
          # The anchor state turns over HERE and not at the `<`: the
          # gem's second clause is `pre.rindex(/<a\b.*?>/i)`, which
          # needs the whole opening tag to be behind the position.
          tag = s[open_lt, i - open_lt + 1].to_s
          in_anchor = true if auto_link_anchor_open?(tag)
          in_anchor = false if auto_link_anchor_close?(tag)
          open_lt = -1
          open_lt_closes = false
          out = out + c
          i = i + 1
        elsif in_anchor || (open_lt >= 0 && open_lt_closes && i >= open_lt + 2)
          out = out + c
          i = i + 1
        else
          len = do_urls ? auto_link_url_length(s, i) : 0
          if len > 0
            out = out + auto_link_url(s[i, len].to_s, html)
            i = i + len
          else
            len = do_emails ? auto_link_email_length(s, i) : 0
            if len > 0
              out = out + mail_to(s[i, len].to_s, "", html)
              i = i + len
            else
              out = out + c
              i = i + 1
            end
          end
        end
      end
      out
    end

    # `<a`, `<A`, `<a href=...` — but not `<abbr`. The gem's test is
    # `/<a\b.*?>/i`, so the character after the name must not be a word
    # one.
    def self.auto_link_anchor_open?(tag)
      return false unless tag[0, 2].to_s.downcase == "<a"
      c = tag[2, 1].to_s
      c == ">" || !auto_link_word_char?(c)
    end

    def self.auto_link_anchor_close?(tag)
      tag.downcase == "</a>"
    end

    # Length of the URL match starting at `i`, or 0. Two openings, the
    # gem's: a listed scheme followed by `://`, or `www.` and a word
    # character. Both are case-insensitive (`/i`), and both are then
    # followed by one or more characters that are not whitespace, `<` or
    # `"` — which is what stops a link at the tag that follows it.
    def self.auto_link_url_length(s, i)
      n = s.length
      j = auto_link_url_prefix_end(s, i)
      return 0 if j == 0
      k = j
      while k < n && auto_link_url_char?(s[k, 1].to_s)
        k = k + 1
      end
      return 0 if k == j
      k - i
    end

    # Index just past `scheme://` or `www.` + one word char, or 0 if
    # neither opens at `i`.
    def self.auto_link_url_prefix_end(s, i)
      if s[i, 4].to_s.downcase == "www." && auto_link_word_char?(s[i + 4, 1].to_s)
        return i + 5
      end
      colon = s.index(":", i)
      return 0 if colon.nil?
      return 0 unless s[colon, 3].to_s == "://"
      return 0 unless auto_link_scheme?(s[i, colon - i].to_s.downcase)
      colon + 3
    end

    # The gem's scheme list, delimited so a prefix cannot match a longer
    # name (`ftp` must not answer for `sftp`). Inline literal rather than
    # a module const: a module-const receiver reads as an unresolved
    # class in the strict typer, the same reason `tag_open_at?` above
    # spells its alphabet out.
    def self.auto_link_scheme?(name)
      return false if name.empty?
      "|ed2k|ftp|http|https|irc|mailto|news|gopher|nntp|telnet|webcal|" \
      "xmpp|callto|feed|svn|urn|aim|rsync|tag|ssh|sftp|rtsp|afs|file|"
        .include?("|" + name + "|")
    end

    # `[^\s< "]` — Ruby's `\s` is exactly `[ \t\r\n\f\v]` here.
    def self.auto_link_url_char?(c)
      return false if c == ""
      return false if c == " " || c == "\t" || c == "\r" || c == "\n"
      return false if c == "\f" || c == "\v"
      c != "<" && c != "\""
    end

    # `\p{Word}` — Unicode letters, marks, numbers and connector
    # punctuation. ASCII is spelled out; everything above it is TAKEN as
    # a word character, which is where this parts company with the gem.
    # A URL ending in a non-ASCII letter (the common case) agrees; one
    # ending in non-ASCII PUNCTUATION (`»`, `。`) keeps the character
    # here and drops it there. Ledgered with the rest.
    def self.auto_link_word_char?(c)
      return false if c == ""
      return true if c.ord >= 128
      "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_".include?(c)
    end

    # Build the anchor for one URL match, trailing punctuation and all.
    #
    #   while href.sub!(/[^\p{Word}\/\-=;]$/, "")
    #     punctuation.push($&)
    #     opening = BRACKETS[punctuation.last]
    #     if opening && href.scan(opening).size > href.scan($&).size
    #       href << punctuation.pop
    #       break
    #     end
    #   end
    #
    # is the gem, and the bracket clause is the whole point of it: a
    # sentence's full stop is not part of the URL, but the `)` that
    # closes a Wikipedia title IS, and the two are told apart by whether
    # the URL opened one.
    def self.auto_link_url(matched, html)
      href = matched
      punctuation = []
      while href.length > 0 && !auto_link_url_tail_char?(href[href.length - 1, 1].to_s)
        last = href[href.length - 1, 1].to_s
        href = href[0, href.length - 1].to_s
        punctuation.push(last)
        opening = auto_link_bracket_opening(last)
        if opening != "" && auto_link_count(href, opening) > auto_link_count(href, last)
          # `href << punctuation.pop` in the gem. Split, and the pop is a
          # STATEMENT: used as a value it types as an Option on the
          # strict targets, and `last` is the same character anyway.
          punctuation.pop
          href = href + last
          break
        end
      end
      # `&gt;` survives the loop above (`;` is a keeper) and is not part
      # of the URL either — it is the escaped `>` of the markup around
      # it.
      trailing_gt = ""
      if href.length >= 4 && href[href.length - 4, 4].to_s == "&gt;"
        trailing_gt = "&gt;"
        href = href[0, href.length - 4].to_s
      end
      # The LABEL is the text as written; the `http://` prefix the `www.`
      # branch needs goes only into the href. Getting this backwards
      # rewrites the user's text.
      label = href
      # The gem's condition is `if scheme.nil?`, and scheme is nil for
      # exactly the `www.` branch — no listed scheme name is "www".
      href = "http://" + href if href[0, 4].to_s.downcase == "www."
      # The gem runs `href` and the label through `sanitize` here, and
      # then hands `content_tag` an `escape` flag. NEITHER is reproduced,
      # and neither needs to be:
      #
      #   * `sanitize` on this text is not a filtering question — the
      #     match is `[^\s< "]+`, so there is no markup in it — it is an
      #     ENTITY question, and the gem never sees a bare `&` here
      #     because the body pass already turned it into `&amp;`. With
      #     that pass skipped, one follows from the other: a bare `&` in
      #     the input reaches the href as written where Rails would have
      #     `&amp;`. campfire's body arrives through `h`, so its `&` is
      #     already an entity. Ledgered with the body pass.
      #   * `escape` is `content_tag`'s fourth argument, and it is a
      #     no-op in the gem for the reason above: it is true only when
      #     the sanitize ran, and a sanitized value is html_safe, which
      #     `content_tag` splices raw. Confirmed against the gem on all
      #     three settings of the flag.
      auto_link_anchor(href, label, html) + punctuation.reverse.join("") + trailing_gt
    end

    # `[^\p{Word}\/\-=;]$` inverted: the characters a URL may END on.
    def self.auto_link_url_tail_char?(c)
      auto_link_word_char?(c) || c == "/" || c == "-" || c == "=" || c == ";"
    end

    # `{ "]" => "[", ")" => "(", "}" => "{" }`, as a lookup with "" for
    # "not a closing bracket".
    def self.auto_link_bracket_opening(c)
      return "[" if c == "]"
      return "(" if c == ")"
      return "{" if c == "}"
      ""
    end

    def self.auto_link_count(s, c)
      count = 0
      i = 0
      n = s.length
      while i < n
        count = count + 1 if s[i, 1].to_s == c
        i = i + 1
      end
      count
    end

    # `content_tag(:a, text, attrs.merge("href" => href))`, spelled out
    # because both halves of that call diverge from the shared helpers:
    # `content_tag` ESCAPES its content and `render_attrs` escapes its
    # values, and here the label and the href are already escaped text
    # from the body (Rails reaches the same place by way of html_safe).
    # `merge` also REPLACES an `href` the caller passed rather than
    # appending after it, which is the attribute order the golden values
    # in `tests/overlay_sanitize_autolink.rb` pin.
    def self.auto_link_anchor(href, text, html)
      attrs = +""
      placed = false
      html.each do |k, v|
        if k.to_s == "href"
          attrs = attrs + " href=\"" + href + "\""
          placed = true
        else
          attrs = attrs + " " + k.to_s + "=\"" + html_escape(v.to_s) + "\""
        end
      end
      attrs = attrs + " href=\"" + href + "\"" unless placed
      "<a" + attrs + ">" + text + "</a>"
    end

    # Length of the e-mail match starting at `i`, or 0. The gem's
    # pattern, left to right: a lookbehind that refuses a start in the
    # MIDDLE of a local part, one starting character from the narrower
    # set, an optional dot, the rest of the local part, `@`, and a
    # domain of two or more `[\w-]` labels.
    def self.auto_link_email_length(s, i)
      n = s.length
      return 0 if i > 0 && auto_link_email_local_char?(s[i - 1, 1].to_s)
      return 0 unless auto_link_email_start_char?(s[i, 1].to_s)
      j = i + 1
      j = j + 1 if s[j, 1].to_s == "."
      while j < n && auto_link_email_local_char?(s[j, 1].to_s)
        j = j + 1
      end
      return 0 unless s[j, 1].to_s == "@"
      j = j + 1
      k = j
      while k < n && auto_link_email_domain_char?(s[k, 1].to_s)
        k = k + 1
      end
      return 0 if k == j
      # `(?:\.[\w-]+)+` — one label is not a domain.
      labels = 0
      more = true
      while more
        more = false
        if s[k, 1].to_s == "."
          m = k + 1
          while m < n && auto_link_email_domain_char?(s[m, 1].to_s)
            m = m + 1
          end
          if m > k + 1
            k = m
            labels = labels + 1
            more = true
          end
        end
      end
      return 0 if labels == 0
      k - i
    end

    # `[\w.!#\$%&'*\/=?^`{|}~+-]`
    def self.auto_link_email_local_char?(c)
      return false if c == ""
      auto_link_word_char?(c) || ".!\#$%&'*/=?^`{|}~+-".include?(c)
    end

    # `[\w.!#\$%+-]` — the narrower set the local part may OPEN with.
    def self.auto_link_email_start_char?(c)
      return false if c == ""
      auto_link_word_char?(c) || ".!\#$%+-".include?(c)
    end

    # `[\w-]`
    def self.auto_link_email_domain_char?(c)
      return false if c == ""
      auto_link_word_char?(c) || c == "-"
    end
    # Rails' CaptureHelper, for the block a helper FORWARDS rather than
    # writes. `src/lower/capture_inline.rs` claims the LITERAL-block
    # shape (`capture { concat(a); … }`) and inlines it into an
    # accumulator — buffer, `concat` sites and all. What reaches here is
    # the other shape: campfire's `ClipboardHelper
    # .button_to_copy_to_clipboard(url, &)` forwards its caller's block
    # into `tag.button`, and the lowered tag calls `capture(&__blk)`
    # with a block it cannot see. There is nothing to inline, so the
    # call has to land on a real method.
    #
    # It answers the block's own value when that value is a String —
    # Rails' `buffer.presence || value`, of which this is the second
    # half. The first half cannot arrive: an emitted block builds its
    # markup and RETURNS it (`_cap` in a view, `a + b` in a helper), and
    # a `concat` that would have filled a buffer instead was rewritten
    # into an append by the pass above. A non-String value is NOT
    # stringified — Rails answers the empty buffer there, and a helper
    # block ending on an Integer means its markup went somewhere else.
    #
    # MOVED here from the CRuby overlay (`ruby_overlay/runtime/
    # action_view_capture_helper.rb`, deleted) rather than copied: the
    # overlay ships alongside this file on the CRuby lane, so two
    # definitions would mean two lanes rendering different HTML with
    # require order deciding which. The overlay's buffer STACK did not
    # come with it — `Thread.current` is not a shape every ruby-family
    # lane types, and a stack no `concat` can push to is state the
    # corpus has zero call sites for (`ViewHelpers.concat` appears
    # nowhere in campfire's or lobsters' emit).
    def self.capture
      value = yield
      value.is_a?(String) ? value.to_s : ""
    end

    # Rails appends to the view's output buffer; emitted views write
    # through `io <<` and have none, so there is nothing to append to.
    # Kept as the loud failure the CRuby overlay made it — a call site
    # that does appear says which one it is, instead of vanishing into a
    # NameError (or, on a strict target, an unresolved-call build wall
    # with no explanation attached).
    def self.concat(string)
      raise "concat outside capture — emitted views buffer through io<<, not concat"
    end
    # `polymorphic_url(record, only_path: true)` — Rails' "the route for
    # whatever this record is", resolved at RUNTIME from the record's
    # class. Nothing here can answer that: it needs a class-to-route
    # registry, which is exactly the dynamic dispatch the strict targets
    # cannot carry, and a record whose class IS known statically never
    # arrives — the url/form lowerings rewrite that site to the
    # generated `<model>_path` helper at compile time, which is where a
    # polymorphic route belongs.
    #
    # So it RAISES, in the same voice and for the same reason as
    # `RouteHelpers.rails_blob_path`: the one call site the corpus has
    # (campfire's `BroadcastsHelper.broadcast_image_path`) hands it an
    # Active Storage representation, and the bytes half of Active
    # Storage — service, processor, signed ids — is unmodeled. A
    # plausible-looking URL would be a page that renders a broken image,
    # the failure that looks like success. What changes is that the gap
    # has ONE named home instead of a method NOTHING defines: a
    # NameError on CRuby, and `unsupported call: (CallNode
    # 'polymorphic_url')` that stops the whole spinel build.
    #
    # HERE rather than in the universal `view_helpers.rb` for this
    # file's own reason: a `raise <Class>, "msg"` body is emitter
    # surface the rust lane does not carry in a RUNTIME unit — its
    # transpiled `src/view_helpers.rs` has no `use crate::errors_ext::
    # {raise, NotImplementedError}` in its header the way a controller
    # unit does, so the stub compiled everywhere else and took
    # `rust_toolchain` from green to two E0425s. It joins the universal
    # file when that header does.
    def self.polymorphic_url(record, only_path: false)
      raise NotImplementedError,
            "ActionView::ViewHelpers.polymorphic_url: a record's route is " \
            "resolved at transpile time — no runtime record-to-route " \
            "mapping is modeled; for the Active Storage case see " \
            "ActiveStorage::Attached#url"
    end
  end
end
