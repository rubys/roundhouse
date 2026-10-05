# Default ActiveRecord JSON entrypoint for the Spinel runtime.
# Always loaded: date-free apps still need `as_json` → `_as_json_only`
# (time-aware via shared connection.rb). Date-column rewrite lives in
# the omit-when-unused sibling `active_record_date_serialization.rb`.
module ActiveRecord
  class Base
    def as_json(options = {})
      only = options && options[:only]
      only ||= self.class.schema_columns
      _as_json_only(only)
    end
  end
end
