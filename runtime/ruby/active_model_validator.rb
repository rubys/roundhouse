# frozen_string_literal: true
# `ActiveModel::Validator`, the base of a `validates_with` class
# (`app/validators/*.rb`): built with the declaration's options, then
# `validate(record)` adds the record's errors. The ruby family's
# `validates_with` expansion (`ingest::validates_with`) calls
# `<Validator>.new(options).validate(self)` from the model's validation.
module ActiveModel
  class Validator
    def initialize(options = {})
      @options = options
    end

    def options
      @options
    end

    # A subclass defines it; the base adds nothing.
    def validate(record)
      nil
    end
  end

  # `ActiveModel::EachValidator`, the base of a `validates :attr, foo: …`
  # class (`FooValidator`): built with the attributes and the key's
  # options, then `validate_each(record, attribute, value)` per attribute.
  # The ruby family's expansion (`ingest::each_validator`) reads each
  # attribute and calls it from the model's validation, skipping nil or
  # blank values under the call's `allow_nil:` / `allow_blank:`.
  class EachValidator < Validator
    # Sets `@options` itself, as `Validator#initialize` does, rather than
    # through `super`, whose call isn't typed.
    def initialize(options = {})
      @attributes = options.fetch(:attributes, [])
      @options = options.reject { |key| key == :attributes }
    end

    def attributes
      @attributes
    end

    # A subclass defines it; the base adds nothing.
    def validate_each(record, attribute, value)
      nil
    end
  end
end
