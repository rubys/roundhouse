# frozen_string_literal: true

# CRuby overlay: ActiveRecord::Base#as_json — the Rails serialization
# default a model's custom `as_json` composes with via `super(only:
# [...])` (lobsters User#as_json does exactly that). Reopens Base so
# instance-method inheritance reaches every model (the find_by!
# precedent in active_record_bang.rb).
#
# Rails' serializable_hash subset: string keys, `only:` narrows the
# attribute list; without `only:` every column attribute serializes
# (public names from the schema, not internal `_raw` storage ivars).
# Date values become ISO date text; other values stay raw here; the
# JsonRender walk (or a custom as_json caller) primitivizes them.
#
# CRuby-only by nature (send/instance_variables reflection) — exactly
# why it lives in the overlay and not runtime/ruby.
module ActiveRecord
  class Base
    def as_json(options = {})
      names =
        if options && options[:only]
          options[:only].map { |n| n.to_s }
        else
          self.class.schema_columns.map { |n| n.to_s }
        end
      h = {}
      names.each do |n|
        # Rails' serializable_hash reads COLUMN attributes only: a
        # requested name with no column storage behind it (a
        # typed_store accessor like lobsters' homepage, which lives
        # inside the settings YAML) is OMITTED — no key, and the
        # store reader is not consulted (verified against the Rails
        # /hottest dump: the user's settings blob holds a homepage
        # value, the JSON has no homepage key). Column storage = the
        # same-named ivar, or the `_raw` ivar the datetime lowering
        # renames storage to.
        next unless instance_variable_defined?("@#{n}") ||
                    instance_variable_defined?("@#{n}_raw")
        if respond_to?(n)
          value = send(n)
          # Date-only JSON is YYYY-MM-DD. DateTime is a distinct
          # timestamp domain despite being a Ruby subclass of Date.
          h[n] = value.instance_of?(Date) ? value.iso8601 : value
        end
      end
      h
    end

    # The shared runtime's super(only:) seam reads raw storage and
    # already handles timestamp JSON. Native Date must instead follow
    # its public reader, including empty-as-absent nonnullable storage.
    alias_method :_as_json_only_without_dates, :_as_json_only
    private :_as_json_only_without_dates

    def _as_json_only(only)
      h = _as_json_only_without_dates(only)
      only.each do |key|
        name = key.to_s
        next unless h.key?(name) && respond_to?(name)
        value = send(name)
        h[name] = value.iso8601 if value.instance_of?(Date)
        h[name] = nil if value.nil?
      end
      h
    end
  end
end
