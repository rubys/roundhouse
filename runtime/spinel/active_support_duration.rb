# ActiveSupport::Duration for the spinel (AOT) tree.
#
# The value class HALF of the CRuby overlay's
# `ruby_overlay/runtime/active_support_duration.rb` — same constants,
# same builders, same seconds-based arithmetic. What it deliberately
# omits is that file's `class Time` reopen (Duration-aware `<=>`/`-`/`+`
# via `alias_method` and Rational math): reopening a built-in's
# operators is CRuby-only forever, and spinel's subset rejects the
# Rational literals it leans on. Sites that need Time±Duration under
# AOT get grounded at emit instead (`.ago`/`.from_now` live here and
# cover the corpus's dominant shape).
#
# On the CRuby/JRuby trees the overlay file replaces this one wholesale
# (same emitted path, dedupe-last-wins), so the two never coexist.
#
# Seconds-based. days/hours/minutes/weeks are exact; months/years use
# Rails' average-length constants (a month is 1/12 of a 365.2425-day
# year) — close enough for the comparisons the corpus makes, not a
# calendar engine.
module ActiveSupport
  class Duration
    SECONDS_PER_MINUTE = 60
    SECONDS_PER_HOUR = 3600
    SECONDS_PER_DAY = 86400
    SECONDS_PER_WEEK = 604800
    SECONDS_PER_MONTH = 2629746
    SECONDS_PER_YEAR = 31556952

    def initialize(seconds)
      @seconds = seconds
    end

    def self.seconds(n)
      new(n)
    end

    def self.second(n)
      new(n)
    end

    def self.minutes(n)
      new(n * SECONDS_PER_MINUTE)
    end

    def self.minute(n)
      new(n * SECONDS_PER_MINUTE)
    end

    def self.hours(n)
      new(n * SECONDS_PER_HOUR)
    end

    def self.hour(n)
      new(n * SECONDS_PER_HOUR)
    end

    def self.days(n)
      new(n * SECONDS_PER_DAY)
    end

    def self.day(n)
      new(n * SECONDS_PER_DAY)
    end

    def self.weeks(n)
      new(n * SECONDS_PER_WEEK)
    end

    def self.week(n)
      new(n * SECONDS_PER_WEEK)
    end

    def self.fortnights(n)
      new(n * SECONDS_PER_WEEK * 2)
    end

    def self.fortnight(n)
      new(n * SECONDS_PER_WEEK * 2)
    end

    def self.months(n)
      new(n * SECONDS_PER_MONTH)
    end

    def self.month(n)
      new(n * SECONDS_PER_MONTH)
    end

    def self.years(n)
      new(n * SECONDS_PER_YEAR)
    end

    def self.year(n)
      new(n * SECONDS_PER_YEAR)
    end

    # A Time minus seconds is a Time, so `ago`/`from_now` (and the
    # Rails aliases `until`/`since`) stay Time-typed for callers like
    # `created_at > 70.days.ago`.
    def ago
      Time.now.utc - @seconds
    end

    def until
      Time.now.utc - @seconds
    end

    def from_now
      Time.now.utc + @seconds
    end

    def since
      Time.now.utc + @seconds
    end

    def to_i
      @seconds.to_i
    end

    def to_f
      @seconds.to_f
    end

    # Rails parity: Duration#to_s is the seconds VALUE's to_s
    # (`30.minutes.to_s == "1800"`). Also what string interpolation
    # calls — lobsters' cache keys embed durations
    # (`"aggregates_#{interval}_#{cache_time}"`), and matching Rails
    # here keeps those keys byte-identical.
    def to_s
      @seconds.to_s
    end

    def seconds
      @seconds
    end
  end
end
