# Date-column JSON rewrite for Spinel. Omit-when-unused with the Date
# package (matz/spinel#7334). Default `as_json` lives in the always-on
# `active_record_serialization.rb`; this reopen wraps `_as_json_only`
# after the shared time-aware seam so date columns become ISO
# `YYYY-MM-DD` (or nil) without taxing shared `connection.rb`.
module ActiveRecord
  class Base
    alias_method :_as_json_only_without_dates, :_as_json_only
    private :_as_json_only_without_dates

    def _as_json_only(only)
      h = _as_json_only_without_dates(only)
      date_columns = self.class.schema_date_columns
      only.each do |k|
        name = k.to_s
        next unless h.key?(name) && date_columns.include?(k)
        h[name] = ActiveSupport.format_db_date(self[k])
      end
      h
    end
  end
end
