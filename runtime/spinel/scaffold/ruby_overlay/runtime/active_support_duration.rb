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

    # Numeric comparison protocol: `Time.current - created_at <=
    # 14.days` has a Float receiver, and Ruby's numeric `<=>` resolves
    # an unknown operand through its `coerce` — pair the duration up as
    # its seconds value (AS Duration is coercible the same way).
    def coerce(other)
      [other, @seconds]
    end
  end
end

# ActiveSupport patches `Time#<=>` (compare_with_coercion) so a non-Time
# operand routes through `to_datetime <=> other`; for a bare Duration that
# resolves to comparing the astronomical julian day NUMBER against the
# duration's bare seconds VALUE (pinned empirically against activesupport
# 8.1: `Time.at(0) <= 2440587.seconds` → false, `<= 2440589.seconds` →
# true — flip exactly at ajd == seconds). Dimensionally meaningless, but
# lobsters ships `created_at <= 1.hour` (story.rb `send_referrer?`, a
# dormant `.ago`-less bug) and on Rails it evaluates — always false for
# realistic timestamps — instead of raising. Mirror the arithmetic so the
# benchmark sees identical behavior. 210_866_760_000 = 2440587.5 * 86400
# (unix epoch's ajd, in seconds).
class Time
  # `Time.current` — AS's zone-aware now. The bench runs zoneless
  # (config.time_zone default UTC, TZ-naive comparisons throughout), so
  # plain `now` is the same instant.
  def self.current
    now
  end

  alias_method :roundhouse_compare_without_duration, :<=>
  def <=>(other)
    if other.is_a?(ActiveSupport::Duration)
      ajd = (to_r + 210_866_760_000r) / 86_400r
      ajd <=> other.seconds
    else
      roundhouse_compare_without_duration(other)
    end
  end

  # Time ± Duration → Time (AS arithmetic; the commentbox preview
  # window does `Time.current - 90.seconds`).
  alias_method :roundhouse_minus_without_duration, :-
  def -(other)
    other = other.seconds if other.is_a?(ActiveSupport::Duration)
    roundhouse_minus_without_duration(other)
  end

  alias_method :roundhouse_plus_without_duration, :+
  def +(other)
    other = other.seconds if other.is_a?(ActiveSupport::Duration)
    roundhouse_plus_without_duration(other)
  end

  # AS's readable comparators (`story.created_at.after?(cutoff)`).
  def after?(other)
    self > other
  end

  def before?(other)
    self < other
  end
end
