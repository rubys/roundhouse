# CRuby-only ActiveSupport::Duration.
#
# Rails adds duration builders to Integer/Numeric (`70.days`, `1.week`) plus
# `Duration#ago` / `#from_now`. Reopening `Integer` in the shared, transpiled
# runtime is off-limits (no built-in subclassing — every strict target would
# have to model it, and `Time` arithmetic doesn't transpile uniformly), so
# the value type lives here and ships only to the CRuby/JRuby trees. The
# Ruby emit path rewrites `<int>.days` → `ActiveSupport::Duration.days(<int>)`
# so no `Integer` method is needed. When lobsters comes up on another target,
# that target gets its own Duration.
#
# Seconds-based. days/hours/minutes/weeks are exact; months/years use Rails'
# average-length constants (a month is 1/12 of a 365.2425-day year), so
# `1.year.ago` is within a fraction of a day of Rails' calendar arithmetic —
# close enough for the comparisons the corpus makes, not a calendar engine.
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

    def self.seconds(n) = new(n)
    def self.second(n) = new(n)
    def self.minutes(n) = new(n * SECONDS_PER_MINUTE)
    def self.minute(n) = new(n * SECONDS_PER_MINUTE)
    def self.hours(n) = new(n * SECONDS_PER_HOUR)
    def self.hour(n) = new(n * SECONDS_PER_HOUR)
    def self.days(n) = new(n * SECONDS_PER_DAY)
    def self.day(n) = new(n * SECONDS_PER_DAY)
    def self.weeks(n) = new(n * SECONDS_PER_WEEK)
    def self.week(n) = new(n * SECONDS_PER_WEEK)
    def self.fortnights(n) = new(n * SECONDS_PER_WEEK * 2)
    def self.fortnight(n) = new(n * SECONDS_PER_WEEK * 2)
    def self.months(n) = new(n * SECONDS_PER_MONTH)
    def self.month(n) = new(n * SECONDS_PER_MONTH)
    def self.years(n) = new(n * SECONDS_PER_YEAR)
    def self.year(n) = new(n * SECONDS_PER_YEAR)

    # A Time minus seconds is a Time in Ruby, so `ago`/`from_now` (and the
    # Rails aliases `until`/`since`) stay Time-typed for callers like
    # `created_at > 70.days.ago`.
    def ago = Time.now - @seconds
    def until = Time.now - @seconds
    def from_now = Time.now + @seconds
    def since = Time.now + @seconds

    def to_i = @seconds.to_i
    def to_f = @seconds.to_f
    def seconds = @seconds
  end
end
