module ActiveRecord
  # Validation helpers callable from a model's `validate` method.
  #
  # Positional value argument: `validates_presence_of(:title, @title)` —
  # the caller passes the current attribute value directly. Avoids
  # `instance_variable_get("@#{attr}")` and `send(attr)` (both rejected
  # by the spinel subset) and avoids the block-yield idiom (which
  # carried generic-block-return type-inference cost). The attribute
  # name is passed for error messages only.
  module Validations
    def errors
      @errors = [] if @errors.nil?
      @errors
    end

    def validates_presence_of(attr_name, value)
      blank = false
      if value.nil?
        blank = true
      elsif value.is_a?(String) && value.empty?
        blank = true
      elsif value.is_a?(Array) && value.empty?
        blank = true
      end
      errors << "#{attr_name} can't be blank" if blank
    end

    def validates_absence_of(attr_name, value)
      present = false
      if !value.nil?
        if value.is_a?(String)
          present = !value.empty?
        elsif value.is_a?(Array)
          present = !value.empty?
        else
          present = true
        end
      end
      errors << "#{attr_name} must be blank" if present
    end

    def validates_length_of(attr_name, value, minimum: nil, maximum: nil, is: nil)
      return if value.nil?
      len = if value.is_a?(String) || value.is_a?(Array)
              value.length
            else
              0
            end
      errors << "#{attr_name} is too short (minimum is #{minimum})" if !minimum.nil? && len < minimum
      errors << "#{attr_name} is too long (maximum is #{maximum})" if !maximum.nil? && len > maximum
      errors << "#{attr_name} is the wrong length (should be #{is})" if !is.nil? && len != is
    end

    def validates_numericality_of(attr_name, value, greater_than: nil, less_than: nil, only_integer: false)
      if value.nil? || !value.is_a?(Numeric)
        errors << "#{attr_name} is not a number"
        return
      end
      errors << "#{attr_name} must be greater than #{greater_than}" if !greater_than.nil? && value <= greater_than
      errors << "#{attr_name} must be less than #{less_than}" if !less_than.nil? && value >= less_than
      errors << "#{attr_name} must be an integer" if only_integer && !value.is_a?(Integer)
    end

    def validates_inclusion_of(attr_name, value, within:)
      errors << "#{attr_name} is not included in the list" unless within.include?(value)
    end

    def validates_format_of(attr_name, value, with:)
      ok = value.is_a?(String) && with.match?(value)
      errors << "#{attr_name} is invalid" unless ok
    end
  end
end
