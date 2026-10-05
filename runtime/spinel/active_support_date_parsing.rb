# Date-only ActiveSupport intrinsics for Spinel. Loaded only with the
# bounded Date package (`app_uses_date`) — Campfire and other date-free
# apps must not see `Date` / `Date?` in the always-on time-parsing seam
# (matz/spinel#7334).
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
    return value.iso8601 if value.is_a?(Date)
    return Date.iso8601(value).iso8601 if value.is_a?(String)
    raise TypeError, "expected Date, String, or nil"
  end
end
