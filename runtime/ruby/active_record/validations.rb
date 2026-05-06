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

    def validates_length_of(attr_name, value, opts = {})
      return if value.nil?
      minimum = opts[:minimum]
      maximum = opts[:maximum]
      is = opts[:is]
      # Single-predicate per branch so the analyzer's narrowing
      # extract (which recognizes one is_a? per branch, not BoolOp
      # chains) types each then-arm cleanly: value: Str → Int,
      # value: Array → Int. Without this shape, `value.length` lands
      # under the BoolOp's untyped `value`, returns Untyped, joins
      # with `0` (Int) into Union<Untyped, Int>, and the downstream
      # `len < minimum` comparison trips a false-positive
      # incompatible-operand diagnostic. The elsif spelling is also
      # plain better Ruby — each branch is unambiguous about which
      # shape it's handling.
      len = if value.is_a?(String)
              value.length
            elsif value.is_a?(Array)
              value.length
            else
              0
            end
      errors << "#{attr_name} is too short (minimum is #{minimum})" if !minimum.nil? && len < minimum
      errors << "#{attr_name} is too long (maximum is #{maximum})" if !maximum.nil? && len > maximum
      errors << "#{attr_name} is the wrong length (should be #{is})" if !is.nil? && len != is
    end

    def validates_numericality_of(attr_name, value, opts = {})
      if value.nil? || !value.is_a?(Numeric)
        errors << "#{attr_name} is not a number"
        return
      end
      greater_than = opts[:greater_than]
      less_than    = opts[:less_than]
      only_integer = opts[:only_integer]
      errors << "#{attr_name} must be greater than #{greater_than}" if !greater_than.nil? && value <= greater_than
      errors << "#{attr_name} must be less than #{less_than}" if !less_than.nil? && value >= less_than
      errors << "#{attr_name} must be an integer" if only_integer && !value.is_a?(Integer)
    end

    def validates_inclusion_of(attr_name, value, opts = {})
      within = opts[:within]
      errors << "#{attr_name} is not included in the list" unless within.include?(value)
    end

    def validates_format_of(attr_name, value, opts = {})
      with = opts[:with]
      ok = value.is_a?(String) && with.match?(value)
      errors << "#{attr_name} is invalid" unless ok
    end

    # `belongs_to` presence — Rails 5+ default. The associated record
    # must exist for the save to succeed. `fk_value` is the foreign-key
    # ivar (e.g. `@article_id`); `target_class` is the model the FK
    # references. Skips the existence query when the FK is unset
    # (nil/0) and emits a "must exist" error so the message matches
    # the parent_name (`article`) rather than the FK name.
    def validates_belongs_to(attr_name, fk_value, target_class)
      if fk_value.nil? || fk_value == 0
        errors << "#{attr_name} must exist"
        return
      end
      errors << "#{attr_name} must exist" unless target_class.exists?(fk_value)
    end
  end
end
