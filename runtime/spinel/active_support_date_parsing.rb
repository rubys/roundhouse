# Date-only ActiveSupport intrinsics for Spinel. Loaded only with the
# bounded Date package (`app_uses_date`) — Campfire and other date-free
# apps must not see `Date` / `Date?` in the always-on time-parsing seam
# (matz/spinel#7334).
#
# Calendar helpers here are the Date-preserving half of
# `active_support_ext.rb`'s Time calendar: Rails' Date receivers answer
# a Date, not a Time. Not a reopen of Date — spinel cannot host one —
# so `lower::time_calendar` grounds Date sends onto these functions.
module ActiveSupport
  # SQL DATE has no clock or zone. The empty string is the SQLite
  # adapter's nil representation for a nullable column.
  def self.parse_db_date(value)
    return nil if value.nil? || value == ""
    Date.iso8601(value)
  end

  def self.format_db_date(value)
    return nil if value.nil?
    return nil if value.is_a?(String) && value == ""
    # `instance_of?(Date)` — DateTime is a Date subclass but carries a
    # clock; a date column stores the civil day only (Rails).
    return value.iso8601 if value.instance_of?(Date)
    if value.is_a?(Date)
      return Date.new(value.year, value.month, value.day).iso8601
    end
    return Date.iso8601(value).iso8601 if value.is_a?(String)
    raise TypeError, "expected Date, String, or nil"
  end

  # Not `ActiveSupport.now` read here: the caller passes it, since this
  # file's typing sees no clock of its own (same rule as today?).
  def self.date_current(now)
    Date.new(now.year, now.month, now.day)
  end

  def self.date_at_midnight(d)
    local_time(d.year, d.month, d.day, 0, 0, 0, 0)
  end

  def self.date_beginning_of_day(date)
    date_at_midnight(date)
  end

  def self.date_end_of_day(date)
    ActiveSupport.local_time(date.year, date.month, date.day, 23, 59, 59, 999_999_999)
  end

  def self.date_from_civil(days)
    t = local_on(days, 0, 0, 0, 0)
    Date.new(t.year, t.month, t.day)
  end

  def self.date_days_since(d, n = 1)
    date_from_civil(civil_days(d.year, d.month, d.day) + n)
  end

  def self.date_days_ago(d, n = 1)
    date_days_since(d, -n)
  end

  def self.date_yesterday(d)
    date_days_since(d, -1)
  end

  def self.date_tomorrow(d)
    date_days_since(d, 1)
  end

  def self.date_weeks_since(d, n = 1)
    date_days_since(d, 7 * n)
  end

  def self.date_weeks_ago(d, n = 1)
    date_days_since(d, -7 * n)
  end

  def self.date_months_since(d, n = 1)
    total = d.year * 12 + d.month - 1 + n
    y = total / 12
    m = total % 12 + 1
    # `days_in_month` from active_support_ext — not `Date.month_length`
    # (Spinel-only); this file is shared with the Ruby-family emit.
    last = days_in_month(y, m)
    Date.new(y, m, d.day > last ? last : d.day)
  end

  def self.date_months_ago(d, n = 1)
    date_months_since(d, -n)
  end

  def self.date_years_since(d, n = 1)
    date_months_since(d, 12 * n)
  end

  def self.date_years_ago(d, n = 1)
    date_months_since(d, -12 * n)
  end

  def self.date_beginning_of_week(d, start = 1)
    date_from_civil(civil_days(d.year, d.month, d.day) - (d.wday + 7 - start) % 7)
  end

  def self.date_end_of_week(d, start = 1)
    date_days_since(date_beginning_of_week(d, start), 6)
  end

  def self.date_next_week(d)
    date_beginning_of_week(date_days_since(d, 7))
  end

  def self.date_prev_week(d)
    date_beginning_of_week(date_days_since(d, -7))
  end

  def self.date_beginning_of_month(d)
    Date.new(d.year, d.month, 1)
  end

  def self.date_end_of_month(d)
    Date.new(d.year, d.month, days_in_month(d.year, d.month))
  end

  def self.date_beginning_of_year(d)
    Date.new(d.year, 1, 1)
  end

  def self.date_end_of_year(d)
    Date.new(d.year, 12, 31)
  end

  # Rails Date#today? / yesterday? / tomorrow? / past? / future? compare
  # calendar days against Date.current — not a midnight Time against now.
  def self.date_today?(d, current)
    d.year == current.year && d.month == current.month && d.day == current.day
  end

  def self.date_yesterday?(d, current)
    date_today?(d, date_yesterday(current))
  end

  def self.date_tomorrow?(d, current)
    date_today?(d, date_tomorrow(current))
  end

  def self.date_past?(d, current)
    civil_days(d.year, d.month, d.day) < civil_days(current.year, current.month, current.day)
  end

  def self.date_future?(d, current)
    civil_days(d.year, d.month, d.day) > civil_days(current.year, current.month, current.day)
  end
end
