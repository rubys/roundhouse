# frozen_string_literal: true

# CRuby overlay: ActiveRecord::Base#as_json — the Rails serialization
# default a model's custom `as_json` composes with via `super(only:
# [...])` (lobsters User#as_json does exactly that). Reopens Base so
# instance-method inheritance reaches every model (the find_by!
# precedent in active_record_bang.rb).
#
# Rails' serializable_hash subset: string keys, `only:` narrows the
# attribute list; without `only:` every column attribute serializes
# (recovered from the model's @-ivars — the synthesized column readers
# store straight into same-named ivars). Values stay raw here; the
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
          instance_variables.map { |iv| iv.to_s.delete_prefix("@") }
        end
      h = {}
      names.each do |n|
        # Rails' serializable_hash reads COLUMN attributes only: a
        # requested name with no column storage behind it (a
        # typed_store accessor like lobsters' homepage, which lives
        # inside the settings YAML) serializes as null — the store
        # reader is NOT consulted (verified against the Rails dump:
        # the user's settings blob holds a homepage value, the JSON
        # says null). Column storage = the same-named ivar, or the
        # `_raw` ivar the datetime lowering renames storage to.
        h[n] =
          if instance_variable_defined?("@#{n}") ||
             instance_variable_defined?("@#{n}_raw")
            send(n) if respond_to?(n)
          end
      end
      h
    end
  end
end
