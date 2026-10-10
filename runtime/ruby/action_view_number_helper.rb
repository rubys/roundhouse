# ActionView's public number-helper facade over the shared ActiveSupport
# formatter. SafeBuffer is not part of the emitted Ruby runtime, so this
# facade returns escaped Strings for valid values; emitted templates use the
# raw template entry point and rely on the normal view-output escaping pass.
require_relative "active_support_number_helper"

module ActionView
  module Helpers
    module NumberHelper
      class InvalidNumberError < StandardError
        def number
          @number
        end

        def message
          "ActionView::Helpers::NumberHelper::InvalidNumberError"
        end

        def initialize(number)
          @number = number
        end
      end

      def self.valid_float_string?(text)
        value = text.strip
        return false if value.empty?
        index = 0
        if value.start_with?("-", "+")
          index = 1
        end
        digits = 0
        decimal = false
        exponent = false
        exponent_digits = 0
        while index < value.length
          char = value[index, 1].to_s
          if char >= "0" && char <= "9"
            if exponent
              exponent_digits = exponent_digits + 1
            else
              digits = digits + 1
            end
          elsif char == "." && !decimal && !exponent
            decimal = true
          elsif (char == "e" || char == "E") && !exponent && digits > 0
            exponent = true
            if index + 1 < value.length && (value[index + 1, 1].to_s == "-" || value[index + 1, 1].to_s == "+")
              index = index + 1
            end
          else
            return false
          end
          index = index + 1
        end
        digits > 0 && (!exponent || exponent_digits > 0)
      end

      def self.valid_number?(number)
        return true if number.is_a?(Integer) || number.is_a?(Float) || number.is_a?(Rational)
        return valid_float_string?(number.to_s) if number.is_a?(Numeric)
        number.is_a?(String) && valid_float_string?(number)
      end

      def self.escape_html(text)
        text.to_s.gsub("&", "&amp;").gsub("<", "&lt;").gsub(">", "&gt;")
          .gsub("\"", "&quot;").gsub("'", "&#39;")
      end

      def self.delegate_number_helper_method(method, number, options = {}, escape_output = true)
        return nil if number.nil?
        normalized = ActiveSupport::NumberHelper.symbolize_options(options)
        raise_on_invalid = normalized.delete(:raise)
        valid = valid_number?(number)
        raise InvalidNumberError, number if raise_on_invalid && !valid
        result = case method
        when :number_to_currency
          ActiveSupport::NumberHelper.number_to_currency(number, normalized)
        when :number_to_human
          ActiveSupport::NumberHelper.number_to_human(number, normalized)
        when :number_to_human_size
          ActiveSupport::NumberHelper.number_to_human_size(number, normalized)
        when :number_to_percentage
          ActiveSupport::NumberHelper.number_to_percentage(number, normalized)
        when :number_to_phone
          ActiveSupport::NumberHelper.number_to_phone(number, normalized)
        when :number_with_delimiter
          ActiveSupport::NumberHelper.number_to_delimited(number, normalized)
        else
          ActiveSupport::NumberHelper.number_to_rounded(number, normalized)
        end
        escape_output && (method == :number_to_phone || valid) ? escape_html(result) : result
      end

      def self.template_number_helper(method, number, options = {})
        delegate_number_helper_method(method, number, options, false)
      end

      def self.number_to_currency(number, options = {})
        delegate_number_helper_method(:number_to_currency, number, options)
      end

      def self.number_to_human(number, options = {})
        delegate_number_helper_method(:number_to_human, number, options)
      end

      def self.number_to_human_size(number, options = {})
        delegate_number_helper_method(:number_to_human_size, number, options)
      end

      def self.number_to_percentage(number, options = {})
        delegate_number_helper_method(:number_to_percentage, number, options)
      end

      def self.number_to_phone(number, options = {})
        delegate_number_helper_method(:number_to_phone, number, options)
      end

      def self.number_with_delimiter(number, options = {})
        delegate_number_helper_method(:number_with_delimiter, number, options)
      end

      def self.number_with_precision(number, options = {})
        delegate_number_helper_method(:number_with_precision, number, options)
      end

    end
  end
end

require_relative "action_view_number_helper_mixin"
