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
  end
end
