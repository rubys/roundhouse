# CRuby-only ActionView date helpers: `time_ago_in_words` /
# `distance_of_time_in_words`.
#
# The bucket walk is Rails' DateHelper#distance_of_time_in_words verbatim
# (actionview/lib/action_view/helpers/date_helper.rb), with the I18n
# lookups collapsed to actionview's built-in :en strings — the corpus runs
# the default locale, and per-route output parity against a real Rails
# lobsters needs the exact wording ("about 1 hour", "less than a minute").
#
# Lives on the CRuby overlay, not the shared runtime: the entry
# computation is `Time - Time`, which doesn't transpile uniformly
# (Crystal yields Time::Span, the JVM wants java.time.Duration) — the
# same boundary that put ActiveSupport::Duration here. When lobsters
# comes up on another target, that target grows its own date-helper
# surface. Callers reach these as `ActionView::ViewHelpers.x(...)` via
# `apply_helper_lowering`'s framework-helper rewrite.
module ActionView
  module ViewHelpers
    MINUTES_IN_YEAR = 525600
    MINUTES_IN_QUARTER_YEAR = 131400
    MINUTES_IN_THREE_QUARTERS_YEAR = 394200

    def self.time_ago_in_words(from_time, include_seconds: false)
      distance_of_time_in_words(from_time, Time.now, include_seconds: include_seconds)
    end

    def self.distance_of_time_in_words(from_time, to_time, include_seconds: false)
      from_time, to_time = to_time, from_time if from_time > to_time
      distance_in_minutes = ((to_time - from_time) / 60.0).round
      distance_in_seconds = (to_time - from_time).round

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
        distance_in_years = minutes_with_offset.div(MINUTES_IN_YEAR)
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
