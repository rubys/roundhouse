module ActiveRecord
  module Validations
    def self.included(base)
      base.extend(ClassMethods)
    end

    module ClassMethods
      def validations
        @validations ||= []
      end

      def inherited(subclass)
        super
        subclass.instance_variable_set(:@validations, validations.dup)
      end

      def validates(*attrs)
        options = attrs.last.is_a?(Hash) ? attrs.pop : {}
        attrs.each do |attr|
          options.each do |check_name, check_options|
            validations << { attribute: attr, check: check_name, options: check_options }
          end
        end
      end
    end

    def errors
      @errors ||= []
    end

    def valid?
      @errors = []
      self.class.validations.each { |v| run_check(v) }
      @errors.empty?
    end

    private

    def run_check(v)
      value = read_attribute_for_validation(v[:attribute])
      case v[:check]
      when :presence
        errors << "#{v[:attribute]} can't be blank" if blank?(value)
      when :absence
        errors << "#{v[:attribute]} must be blank" unless blank?(value)
      when :length
        validate_length(v[:attribute], value, v[:options])
      when :numericality
        validate_numericality(v[:attribute], value, v[:options])
      when :inclusion
        in_set = v[:options].is_a?(Hash) ? v[:options][:in] : v[:options]
        errors << "#{v[:attribute]} is not included in the list" unless in_set.include?(value)
      when :format
        pattern = v[:options].is_a?(Hash) ? v[:options][:with] : v[:options]
        errors << "#{v[:attribute]} is invalid" unless value.is_a?(String) && pattern.match?(value)
      when :uniqueness
        validate_uniqueness(v[:attribute], value, v[:options])
      end
    end

    def read_attribute_for_validation(attr)
      @attributes[attr.to_sym]
    end

    def blank?(value)
      value.nil? || (value.respond_to?(:empty?) && value.empty?)
    end

    def validate_length(attr, value, options)
      options = { is: options } if options.is_a?(Integer)
      return if value.nil?
      len = value.respond_to?(:length) ? value.length : 0
      if options[:minimum] && len < options[:minimum]
        errors << "#{attr} is too short (minimum is #{options[:minimum]})"
      end
      if options[:maximum] && len > options[:maximum]
        errors << "#{attr} is too long (maximum is #{options[:maximum]})"
      end
      if options[:is] && len != options[:is]
        errors << "#{attr} is the wrong length (should be #{options[:is]})"
      end
    end

    def validate_numericality(attr, value, options)
      options = {} unless options.is_a?(Hash)
      if value.nil? || !value.is_a?(Numeric)
        errors << "#{attr} is not a number"
        return
      end
      if options[:greater_than] && !(value > options[:greater_than])
        errors << "#{attr} must be greater than #{options[:greater_than]}"
      end
      if options[:less_than] && !(value < options[:less_than])
        errors << "#{attr} must be less than #{options[:less_than]}"
      end
      if options[:only_integer] && !value.is_a?(Integer)
        errors << "#{attr} must be an integer"
      end
    end

    def validate_uniqueness(attr, value, options)
      options = {} unless options.is_a?(Hash)
      table = self.class.table_name
      scope_attrs = Array(options[:scope])
      matches = ActiveRecord.adapter.all(table).select do |row|
        row_val = row[attr.to_sym]
        same = options[:case_sensitive] == false && row_val.is_a?(String) && value.is_a?(String) ?
          row_val.downcase == value.downcase : row_val == value
        same &&
          (!persisted? || row[:id] != @attributes[:id]) &&
          scope_attrs.all? { |s| row[s.to_sym] == @attributes[s.to_sym] }
      end
      errors << "#{attr} has already been taken" unless matches.empty?
    end
  end
end
