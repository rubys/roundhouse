# Date-aware ActiveRecord JSON for the Spinel runtime.
#
# Kept beside the bounded Date (not in shared `runtime/ruby/`) because:
# - the CRuby/JRuby overlay supplies its own reflection-aware reopen
# - putting a `schema_date_columns` / `format_db_date` branch into the
#   shared `_as_json_only` paid Bar B / AR RBS-probe residuals on every
#   analyze of `connection.rb`, including apps that never load Date
# - Campfire omits this file via `app_uses_date` (matz/spinel#7334)
#
# Mirrors the overlay's posture: call the shared time-aware seam, then
# rewrite date-column values to ISO `YYYY-MM-DD` (or nil).
module ActiveRecord
  class Base
    def as_json(options = {})
      only = options && options[:only]
      only ||= self.class.schema_columns
      _as_json_only(only)
    end

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
