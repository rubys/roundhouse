module ActiveRecord
  # Instance-level validation helpers. Transpiled models call these
  # from their `validate` method.
  module Validations
    def errors
      @errors ||= []
    end

    def validates_presence_of(attr)
      value = @attributes[attr.to_sym]
      errors << "#{attr} can't be blank" if blank?(value)
    end

    def validates_absence_of(attr)
      value = @attributes[attr.to_sym]
      errors << "#{attr} must be blank" unless blank?(value)
    end

    def validates_length_of(attr, minimum: nil, maximum: nil, is: nil)
      value = @attributes[attr.to_sym]
      return if value.nil?
      len = value.respond_to?(:length) ? value.length : 0
      errors << "#{attr} is too short (minimum is #{minimum})" if minimum && len < minimum
      errors << "#{attr} is too long (maximum is #{maximum})" if maximum && len > maximum
      errors << "#{attr} is the wrong length (should be #{is})" if is && len != is
    end

    def validates_numericality_of(attr, greater_than: nil, less_than: nil, only_integer: false)
      value = @attributes[attr.to_sym]
      if value.nil? || !value.is_a?(Numeric)
        errors << "#{attr} is not a number"
        return
      end
      errors << "#{attr} must be greater than #{greater_than}" if greater_than && !(value > greater_than)
      errors << "#{attr} must be less than #{less_than}" if less_than && !(value < less_than)
      errors << "#{attr} must be an integer" if only_integer && !value.is_a?(Integer)
    end

    def validates_inclusion_of(attr, in:)
      set = binding.local_variable_get(:in)
      value = @attributes[attr.to_sym]
      errors << "#{attr} is not included in the list" unless set.include?(value)
    end

    def validates_format_of(attr, with:)
      value = @attributes[attr.to_sym]
      errors << "#{attr} is invalid" unless value.is_a?(String) && with.match?(value)
    end

    def validates_uniqueness_of(attr, scope: [], case_sensitive: true)
      value = @attributes[attr.to_sym]
      table = self.class.table_name
      scope_attrs = Array(scope)
      matches = ActiveRecord.adapter.all(table).select do |row|
        row_val = row[attr.to_sym]
        same = if !case_sensitive && row_val.is_a?(String) && value.is_a?(String)
                 row_val.downcase == value.downcase
               else
                 row_val == value
               end
        same &&
          (!persisted? || row[:id] != @attributes[:id]) &&
          scope_attrs.all? { |s| row[s.to_sym] == @attributes[s.to_sym] }
      end
      errors << "#{attr} has already been taken" unless matches.empty?
    end

    private

    def blank?(value)
      value.nil? || (value.respond_to?(:empty?) && value.empty?)
    end
  end
end
