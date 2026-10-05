# Default ActiveRecord JSON shape for the Spinel runtime. This is kept
# beside the bounded Date rather than in the shared runtime because the
# Ruby-family overlay supplies its own reflection-aware implementation.
module ActiveRecord
  class Base
    def as_json(options = {})
      only = options && options[:only]
      only ||= self.class.schema_columns
      _as_json_only(only)
    end
  end
end
