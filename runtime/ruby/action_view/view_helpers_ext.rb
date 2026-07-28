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
      from_time, to_time = to_time, from_time if from_time > to_time
      # `to_f` rather than Rails' bare `to_time - from_time`: `Time#-`
      # already RETURNS float seconds, so the arithmetic is identical,
      # but receiver-only dispatch can't tell a Duration argument (→
      # Time) from a Time one (→ Float) and so types `Time - x` as
      # untyped. Taking the epoch floats first keeps every expression
      # below concretely Float/Integer, which is what the framework
      # runtime's fully-typed invariant requires.
      elapsed = to_time.to_f - from_time.to_f
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
        from_year = from_time.year
        from_year += 1 if from_time.month >= 3
        to_year = to_time.year
        to_year -= 1 if to_time.month < 3

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
  end
end
