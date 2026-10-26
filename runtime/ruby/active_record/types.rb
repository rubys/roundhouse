# frozen_string_literal: true
# ActiveRecord::Type::String and Type::Integer. Boolean stays on the
# cast lowerer (`ActiveSupport.cast_boolean`); this file does not
# define it. A cast answers nil for a blank value and the typed value
# otherwise. `deserialize` casts the same way.
module ActiveRecord
  module Type
    # What `type_for_attribute` answers: the column's type name.
    class Value
      attr_reader :type

      def initialize(type)
        @type = type
      end
    end

    class String
      def cast(value)
        return nil if value.nil?
        return "t" if value == true
        return "f" if value == false
        value.to_s
      end

      def deserialize(value)
        cast(value)
      end
    end

    class Integer
      def cast(value)
        return nil if value.nil?
        return 1 if value == true
        return 0 if value == false
        return value if value.is_a?(::Integer)
        # `::String`: this module also defines `Type::String`, and a bare
        # `String` constant resolves to that class.
        if value.is_a?(::String)
          return nil if value.strip.empty?
          return value.to_i
        end
        return nil unless value.respond_to?(:to_i)
        value.to_i
      end

      def deserialize(value)
        cast(value)
      end
    end
  end
end
