# ActiveSupport::NumberHelper — shared Ruby-family implementation.
#
# Keeping the formatter here gives CRuby and Spinel one implementation
# rather than separate overlay algorithms.
module ActiveSupport
  module NumberHelper
    DEFAULT_LOCALE = :en

    # A finite translation backend for the Rails number-helper namespace.
    # It accepts concrete translation trees; it deliberately does not
    # pretend to support arbitrary I18n backends, pluralization rules, or
    # runtime locale-file discovery in a compiled Spinel executable.
    @locale_formats = {
      en: {
        format: { separator: ".", delimiter: "," },
        currency: { unit: "$", format: "%u%n", negative_format: "-%u%n", separator: ".", delimiter: ",", precision: 2, significant: false, strip_insignificant_zeros: false },
        percentage: { delimiter: "", format: "%n%", precision: 3 },
        precision: { delimiter: "", precision: 3 },
        human: {
          format: { delimiter: "", precision: 3, significant: true, strip_insignificant_zeros: true },
          storage_units: { format: "%n %u", units: { byte: { one: "Byte", other: "Bytes" }, kb: "KB", mb: "MB", gb: "GB", tb: "TB", pb: "PB", eb: "EB", zb: "ZB" } },
          decimal_units: { format: "%n %u", units: { unit: "", thousand: "Thousand", million: "Million", billion: "Billion", trillion: "Trillion", quadrillion: "Quadrillion" } },
        },
      },
    }

    def self.register_locale(locale, separator, delimiter, translations = {})
      locale = locale.to_sym if locale.is_a?(String)
      profile = { format: { separator: separator, delimiter: delimiter } }
      translations.each { |section, values| profile[section] = values }
      @locale_formats[locale] = profile
    end

    def self.symbolize_options(options)
      normalized = {}
      options.each do |key, value|
        symbol = if key.is_a?(Symbol)
          key
        elsif key.is_a?(String)
          key.to_sym
        else
          raise ArgumentError, "number option keys must be Symbols or Strings"
        end
        normalized[symbol] = value
      end
      normalized
    end

    def self.locale_profile(options)
      locale = options.fetch(:locale, DEFAULT_LOCALE)
      locale = DEFAULT_LOCALE if locale.nil?
      locale = locale.to_sym if locale.is_a?(String)
      profile = @locale_formats[locale]
      raise ArgumentError, "unsupported number locale: #{locale}" if profile.nil?
      profile
    end

    def self.locale_format(options)
      locale_profile(options)[:format]
    end

    def self.translated_option(profile, namespace, key, fallback)
      section = profile[namespace]
      return fallback unless section.is_a?(Hash)
      values = section[key]
      values.nil? ? fallback : values
    end

    def self.validate_options(options, allowed)
      options.each do |key, value|
        value
        unless key.is_a?(Symbol) && allowed.include?(key)
          raise ArgumentError, "unknown number option: #{key}"
        end
      end
      options
    end

    def self.number_to_currency(number, options = {})
      return nil if number.nil?
      options = symbolize_options(options)
      allowed = [:locale, :precision, :unit, :separator, :delimiter, :format,
                 :negative_format, :significant, :strip_insignificant_zeros, :round_mode]
      validate_options(options, allowed)
      profile = locale_profile(options)
      unit = options.fetch(:unit, translated_option(profile, :currency, :unit, "$"))
      precision = options[:precision]
      precision = translated_option(profile, :currency, :precision, 2) if precision.nil?
      format_string = options.fetch(:format, translated_option(profile, :currency, :format, "%u%n"))
      default_negative_format = translated_option(profile, :currency, :negative_format, "-%u%n")
      default_negative_format = "-#{format_string}" if options.key?(:format)
      negative_format = options.fetch(:negative_format, default_negative_format)
      rounded_options = {
        locale: options[:locale], precision: precision,
        separator: options.fetch(:separator, translated_option(profile, :currency, :separator, ".")),
        delimiter: options.fetch(:delimiter, translated_option(profile, :currency, :delimiter, ",")),
        significant: options.fetch(:significant, false),
        strip_insignificant_zeros: options.fetch(:strip_insignificant_zeros, false),
        round_mode: options.fetch(:round_mode, :default),
      }
      amount = number_to_rounded(number, rounded_options)
      selected_format = amount.start_with?("-") ? negative_format : format_string
      selected_format.gsub("%n", amount.start_with?("-") ? amount[1, amount.length - 1].to_s : amount).gsub("%u", unit)
    end

    def self.number_to_percentage(number, options = {})
      return nil if number.nil?
      options = symbolize_options(options)
      validate_options(options, [:locale, :precision, :separator, :delimiter, :format,
                                 :significant, :strip_insignificant_zeros, :round_mode])
      profile = locale_profile(options)
      precision = options.fetch(:precision, translated_option(profile, :percentage, :precision, 3))
      amount = number_to_rounded(number, {
        locale: options[:locale], precision: precision,
        separator: options.fetch(:separator, translated_option(profile, :percentage, :separator,
                                                                  translated_option(profile, :format, :separator, "."))),
        delimiter: options.fetch(:delimiter, translated_option(profile, :percentage, :delimiter, "")),
        significant: options.fetch(:significant, false),
        strip_insignificant_zeros: options.fetch(:strip_insignificant_zeros, false),
        round_mode: options.fetch(:round_mode, :default),
      })
      format_string = options.fetch(:format, translated_option(profile, :percentage, :format, "%n%"))
      format_string.gsub("%n", amount)
    end

    def self.number_to_human(number, options = {})
      return nil if number.nil?
      options = symbolize_options(options)
      validate_options(options, [:locale, :precision, :significant, :strip_insignificant_zeros,
                                 :separator, :delimiter, :format, :units, :round_mode])
      return number.to_s if number.is_a?(String) && !valid_decimal_string?(number)
      profile = locale_profile(options)
      human = profile[:human]
      human_format = human[:format]
      precision = options.fetch(:precision, human_format[:precision])
      significant = options.fetch(:significant, human_format[:significant])
      strip_zeros = options.fetch(:strip_insignificant_zeros, human_format[:strip_insignificant_zeros])
      numeric = number_to_rounded(number, {
        locale: options[:locale], precision: precision, significant: significant,
        strip_insignificant_zeros: strip_zeros, delimiter: "", separator: ".",
        round_mode: options.fetch(:round_mode, :default),
      })
      actual_exponent = decimal_digit_count(numeric) - 1
      units = options[:units]
      unit_map = if units.is_a?(Hash)
        units
      elsif units.nil?
        human[:decimal_units][:units]
      elsif units.is_a?(Symbol) || units.is_a?(String)
        registered = registered_units(profile, units)
        raise ArgumentError, "unregistered number unit scope: #{units}" if registered.nil?
        registered
      else
        raise ArgumentError, ":units must be a Hash or a registered translation scope"
      end
      supported_unit_keys = [:unit, :ten, :hundred, :thousand, :million, :billion,
                             :trillion, :quadrillion, :deci, :centi, :mili, :micro,
                             :nano, :pico, :femto]
      unit_keys = unit_map.keys
      unit_index = 0
      while unit_index < unit_keys.length
        key = unit_keys[unit_index]
        unless supported_unit_keys.include?(key)
          raise ArgumentError, "unsupported number unit: #{key}"
        end
        unit_index = unit_index + 1
      end
      exponent_names = { :unit => 0, :ten => 1, :hundred => 2, :thousand => 3,
                         :million => 6, :billion => 9, :trillion => 12, :quadrillion => 15,
                         :deci => -1, :centi => -2, :mili => -3, :micro => -6,
                         :nano => -9, :pico => -12, :femto => -15 }
      selected_key = :unit
      selected_exponent = actual_exponent < 0 ? -16 : 0
      unit_index = 0
      while unit_index < unit_keys.length
        key = unit_keys[unit_index]
        candidate = exponent_names[key]
        if !candidate.nil? && candidate <= actual_exponent && candidate >= selected_exponent
          selected_key = key
          selected_exponent = candidate
        end
        unit_index = unit_index + 1
      end
      if selected_exponent == -16
        selected_key = :unit
        selected_exponent = 0
      end
      unit_value = unit_map[selected_key]
      unit = if unit_value.is_a?(Hash)
        quantity = unit_value[number.to_i.abs == 1 ? :one : :other]
        quantity.nil? ? unit_value[:other].to_s : quantity.to_s
      else
        unit_value.to_s
      end
      unit = "" if unit_map[selected_key].nil?
      exponent = selected_exponent
      shifted = shift_decimal(numeric, 0 - exponent)
      amount = number_to_rounded(shifted, {
        locale: options[:locale], precision: precision,
        significant: significant, strip_insignificant_zeros: strip_zeros,
        separator: options.fetch(:separator, translated_option(profile, :format, :separator, ".")),
        delimiter: options.fetch(:delimiter, human_format[:delimiter]),
        round_mode: options.fetch(:round_mode, :default),
      })
      format_string = options.fetch(:format, human[:decimal_units][:format])
      format_string.gsub("%n", amount).gsub("%u", unit).strip
    end

    def self.registered_units(profile, scope)
      path = scope.to_s.split(".")
      path = path[1, path.length - 1].to_a if path[0] == "number"
      value = profile
      path.each do |segment|
        return nil unless value.is_a?(Hash)
        value = value[segment.to_sym]
      end
      value.is_a?(Hash) ? value : nil
    end

    def self.number_to_human_size(number, options = {})
      return nil if number.nil?
      options = symbolize_options(options)
      validate_options(options, [:locale, :precision, :significant, :strip_insignificant_zeros,
                                 :separator, :delimiter, :round_mode])
      return number.to_s if number.is_a?(String) && !valid_decimal_string?(number)
      profile = locale_profile(options)
      human = profile[:human]
      storage = human[:storage_units]
      base = storage[:units]
      bytes = numeric_text(number)
      integer_text = bytes.split(".", 2)[0].to_s
      negative = integer_text.start_with?("-")
      magnitude = negative ? integer_text[1, integer_text.length - 1].to_s : integer_text
      magnitude = magnitude.sub(/\A0+/, "")
      magnitude = "0" if magnitude.empty?
      exponent = 0
      while (magnitude.length > 4 || (magnitude.length == 4 && magnitude >= "1024")) && exponent < 7
        magnitude = divide_decimal(magnitude, 1024, 0).split(".", 2)[0].to_s
        magnitude = magnitude.sub(/\A0+/, "")
        magnitude = "0" if magnitude.empty?
        exponent = exponent + 1
      end
      suffixes = [:byte, :kb, :mb, :gb, :tb, :pb, :eb, :zb]
      unit_key = suffixes[exponent]
      unit_value = base[unit_key]
      unit = if unit_value.is_a?(Hash)
        quantity_key = !negative && magnitude == "1" && exponent == 0 ? :one : :other
        quantity = unit_value[quantity_key]
        quantity.nil? ? unit_value[:other].to_s : quantity.to_s
      else
        unit_value.to_s
      end
      amount = ""
      if exponent == 0
        amount = integer_text.to_i.to_s
      else
        precision = options.fetch(:precision, human[:format][:precision])
        digits_after_point = precision.is_a?(Integer) ? precision.abs + 10 : 40
        shifted = bytes
        power = 0
        while power < exponent
          shifted = divide_decimal(shifted, 1024, digits_after_point)
          power = power + 1
        end
        amount = number_to_rounded(shifted, {
          locale: options[:locale], precision: precision,
          significant: options.fetch(:significant, human[:format][:significant]),
          strip_insignificant_zeros: options.fetch(:strip_insignificant_zeros,
                                                   human[:format][:strip_insignificant_zeros]),
          separator: options.fetch(:separator, translated_option(profile, :format, :separator, ".")),
          delimiter: options.fetch(:delimiter, human[:format][:delimiter]),
          round_mode: options.fetch(:round_mode, :default),
        })
      end
      storage[:format].gsub("%n", amount).gsub("%u", unit).strip
    end

    def self.number_to_phone(number, options = {})
      return nil if number.nil?
      options = symbolize_options(options)
      validate_options(options, [:area_code, :delimiter, :country_code, :extension, :pattern])
      digits = number.to_s
      extension = options[:extension]
      extension_text = extension.nil? ? "" : " x #{extension}"
      country = options[:country_code]
      country_text = country.nil? || country.to_s.strip.empty? ? "" : "+#{country}#{options.fetch(:delimiter, "-")}"
      delimiter = options.fetch(:delimiter, "-")
      if options[:area_code]
        pattern = options.fetch(:pattern, /(\d{1,3})(\d{3})(\d{4}$)/)
        digits = digits.gsub(pattern, "(\\1) \\2#{delimiter}\\3")
      else
        pattern = options.fetch(:pattern, /(\d{0,3})(\d{3})(\d{4})$/)
        matched = digits[pattern, 0]
        unless matched.nil?
          first = digits[pattern, 1].to_s
          second = digits[pattern, 2].to_s
          third = digits[pattern, 3].to_s
          digits = first.empty? ? second + delimiter + third : first + delimiter + second + delimiter + third
        end
      end
      country_text + digits + extension_text
    end

    def self.divide_decimal(text, divisor, digits_after_point)
      value = expand_decimal(text.to_s)
      sign = ""
      if value.start_with?("-", "+")
        sign = "-" if value[0, 1].to_s == "-"
        value = value[1, value.length - 1].to_s
      end
      parts = value.split(".", 2)
      whole = parts[0].to_s
      fraction = parts.length > 1 ? parts[1].to_s : ""
      quotient = +""
      remainder = 0
      i = 0
      while i < whole.length
        current = remainder * 10 + whole[i, 1].to_s.to_i
        quotient = quotient + (current / divisor).to_s
        remainder = current % divisor
        i = i + 1
      end
      quotient = quotient.sub(/\A0+/, "")
      quotient = "0" if quotient.empty?
      result = quotient + "."
      i = 0
      while i < digits_after_point && (i < fraction.length || remainder != 0)
        source_digit = i < fraction.length ? fraction[i, 1].to_s.to_i : 0
        current = remainder * 10 + source_digit
        digit = current / divisor
        remainder = current % divisor
        result = result + digit.to_s
        i = i + 1
      end
      sign + result
    end

    def self.shift_decimal(text, places)
      value = expand_decimal(text.to_s)
      sign = ""
      if value.start_with?("-", "+")
        sign = "-" if value[0, 1].to_s == "-"
        value = value[1, value.length - 1].to_s
      end
      parts = value.split(".", 2)
      whole = parts[0].to_s
      fraction = parts.length > 1 ? parts[1].to_s : ""
      point = whole.length + places
      digits = whole + fraction
      if point <= 0
        sign + "0." + ("0" * (0 - point)) + digits
      elsif point >= digits.length
        out = digits
        while out.length < point
          out = out + "0"
        end
        sign + out
      else
        sign + digits[0, point].to_s + "." + digits[point, digits.length - point].to_s
      end
    end

    # Rails number_to_delimited core, including the locale defaults and
    # documented delimiter_pattern hook. Grouping is by three digits when
    # no custom pattern is supplied.
    def self.number_to_delimited(number, options = {})
      return nil if number.nil?
      options = symbolize_options(options)
      validate_options(options, [:locale, :separator, :delimiter, :delimiter_pattern])
      return number.to_s if number.is_a?(String) && !valid_decimal_string?(number)
      formats = locale_format(options)
      separator = options[:separator] || formats[:separator]
      delimiter = options[:delimiter] || formats[:delimiter]
      value = number.to_s
      sign = ""
      if value.start_with?("-", "+")
        sign = value[0, 1].to_s
        value = value[1, value.length - 1].to_s
      end
      parts = value.split(".")
      integer = parts[0].to_s
      pattern = options[:delimiter_pattern]
      if options.key?(:delimiter_pattern) && !pattern.nil?
        unless pattern.is_a?(Regexp)
          raise ArgumentError, "delimiter_pattern must be a Regexp"
        end
        integer = integer.gsub(pattern, "\\0" + delimiter)
      else
        grouped = +""
        i = integer.length
        while i > 3
          grouped = delimiter + integer[i - 3, 3].to_s + grouped
          i = i - 3
        end
        integer = integer[0, i].to_s + grouped
      end
      result = sign + integer
      result = result + separator + parts[1].to_s if parts.length > 1
      result
    end

    # Decimal presentation shared by rounded/currency/percentage helpers.
    def self.number_to_rounded(number, options = {})
      return nil if number.nil?
      if number.is_a?(String)
        number = number.strip if valid_decimal_string?(number)
      end
      options = symbolize_options(options)
      validate_options(options, [:locale, :precision, :round_mode, :significant,
                                 :separator, :delimiter, :strip_insignificant_zeros])
      return number.to_s if number.is_a?(String) && !valid_decimal_string?(number)
      formats = locale_format(options)
      precision = options.fetch(:precision, 3)
      unless precision.nil? || precision.is_a?(Integer)
        raise ArgumentError, "precision must be an Integer or nil"
      end
      if number.is_a?(Rational)
        rational_parts = number.to_s.split("/", 2)
        number = divide_decimal(rational_parts[0].to_s, rational_parts[1].to_i,
                                (precision.nil? ? 40 : precision.abs + 10))
      end
      fixed = ""
      if precision.nil?
        fixed = number.to_s
      else
        requested_precision = precision
        significant = options[:significant] || false
        mode = options.fetch(:round_mode, :default)
        mode = mode.to_sym if mode.is_a?(String)
        unless [:default, :up, :down, :ceiling, :floor, :half_up, :half_down, :half_even].include?(mode)
          raise ArgumentError, "unsupported round_mode: #{mode}"
        end
        if significant && precision > 0
          precision = precision - decimal_digit_count(number.to_s)
        end
        fixed = round_decimal(number.to_s, precision, mode)
        if significant && requested_precision > 0
          rounded_digit_count = decimal_digit_count(fixed)
          display_precision = requested_precision - rounded_digit_count
          display_precision = 0 if display_precision < 0
          fixed = round_decimal(fixed, display_precision, :down)
          precision = display_precision
        end
      end
      decimal = options[:separator] || formats[:separator]
      delimiter = options[:delimiter] || ""
      pieces = fixed.split(".")
      integer = pieces[0].to_s
      fraction = pieces.length > 1 ? pieces[1].to_s : ""
      if options[:strip_insignificant_zeros]
        fraction = fraction.sub(/0+\z/, "")
      end
      grouped = number_to_delimited(integer + (fraction.empty? ? "" : "." + fraction),
                                    locale: options[:locale], delimiter: delimiter,
                                    separator: decimal)
      grouped
    end

    def self.expand_decimal(text)
      parts = text.downcase.split("e", 2)
      exponent = parts.length > 1 ? parts[1].to_i : 0
      value = parts[0]
      sign = ""
      if value.start_with?("-", "+")
        sign = "-" if value[0, 1].to_s == "-"
        value = value[1, value.length - 1].to_s
      end
      number_parts = value.split(".", 2)
      whole = number_parts[0].to_s
      fraction = number_parts.length > 1 ? number_parts[1].to_s : ""
      digits = whole + fraction
      point = whole.length + exponent
      if point <= 0
        sign + "0." + ("0" * (0 - point)) + digits
      elsif point >= digits.length
        expanded = digits
        while expanded.length < point
          expanded = expanded + "0"
        end
        sign + expanded
      else
        sign + digits[0, point].to_s + "." + digits[point, digits.length - point].to_s
      end
    end

    def self.numeric_text(number, digits_after_point = 40)
      if number.is_a?(Rational)
        rational_parts = number.to_s.split("/", 2)
        divide_decimal(rational_parts[0].to_s, rational_parts[1].to_i, digits_after_point)
      else
        expand_decimal(number.to_s)
      end
    end

    def self.valid_decimal_string?(text)
      value = text.strip
      return false if value.empty?
      return false if value.include?("e") || value.include?("E") || value.include?("d") || value.include?("D")
      index = 0
      if value.start_with?("-", "+")
        index = 1
      end
      digits = 0
      decimal = false
      while index < value.length
        char = value[index, 1].to_s
        if char >= "0" && char <= "9"
          digits = digits + 1
        elsif char == "." && !decimal
          decimal = true
        else
          return false
        end
        index = index + 1
      end
      digits > 0
    end

    def self.decimal_digit_count(text)
      signless = text.start_with?("-", "+") ? text[1, text.length - 1].to_s : text
      pieces = signless.downcase.split("e", 2)
      exponent = pieces.length > 1 ? pieces[1].to_i : 0
      number_parts = pieces[0].split(".", 2)
      digits = number_parts[0].to_s + (number_parts.length > 1 ? number_parts[1].to_s : "")
      point = number_parts[0].to_s.length + exponent
      first = 0
      while first < digits.length && digits[first, 1] == "0"
        first = first + 1
      end
      first == digits.length ? 1 : point - first
    end

    def self.round_decimal(text, precision, mode)
      sign = ""
      value = text
      if value.start_with?("-", "+")
        sign = "-" if value[0, 1].to_s == "-"
        value = value[1, value.length - 1].to_s
      end
      pieces = value.downcase.split("e", 2)
      exponent = pieces.length > 1 ? pieces[1].to_i : 0
      number_parts = pieces[0].split(".", 2)
      whole = number_parts[0].to_s
      fraction = number_parts.length > 1 ? number_parts[1].to_s : ""
      digits = whole + fraction
      point = whole.length + exponent
      while digits.length > 1 && digits.start_with?("0")
        digits = digits[1, digits.length - 1].to_s
        point = point - 1
      end
      if digits.gsub("0", "") == ""
        digits = "0"
        point = 1
        sign = ""
      end
      cutoff = point + precision
      kept = ""
      discarded = ""
      original_kept_length = cutoff
      if cutoff <= 0
        kept = "0"
        discarded = ("0" * (0 - cutoff)) + digits
      elsif cutoff >= digits.length
        kept = digits
        while kept.length < cutoff
          kept = kept + "0"
        end
      else
        kept = digits[0, cutoff].to_s
        discarded = digits[cutoff, digits.length - cutoff].to_s
      end
      increment = should_increment(discarded, kept, sign, mode)
      if increment
        kept = increment_digits(kept)
      end
      point = point + (kept.length - original_kept_length) if increment
      digits = kept
      if digits.to_i == 0
        point = 1
        sign = ""
      end
      result = +""
      if point <= 0
        result = "0."
        i = 0
        while i < (0 - point)
          result = result + "0"
          i = i + 1
        end
        result = result + digits
      elsif point >= digits.length
        result = digits
        while result.length < point
          result = result + "0"
        end
      else
        result = digits[0, point].to_s + "." + digits[point, digits.length - point].to_s
      end
      if precision > 0
        if !result.include?(".")
          result = result + "."
        end
        fraction_part = result.split(".", 2)[1].to_s
        while fraction_part.length < precision
          result = result + "0"
          fraction_part = fraction_part + "0"
        end
      end
      sign + result
    end

    def self.should_increment(discarded, kept, sign, mode)
      nonzero = discarded.gsub("0", "") != ""
      case mode
      when :up then nonzero
      when :down then false
      when :ceiling then sign != "-" && nonzero
      when :floor then sign == "-" && nonzero
      when :half_down
        first = discarded[0, 1].to_i
        first > 5 || (first == 5 && discarded[1, discarded.length - 1].to_s.gsub("0", "") != "")
      when :half_even
        first = discarded[0, 1].to_i
        tail = discarded[1, discarded.length - 1].to_s.gsub("0", "") != ""
        first > 5 || (first == 5 && (tail || kept[-1, 1].to_i.odd?))
      else
        discarded[0, 1].to_i >= 5
      end
    end

    def self.increment_digits(digits)
      out = +""
      carry = true
      i = digits.length - 1
      while i >= 0
        digit = digits[i, 1].to_i
        if carry && digit == 9
          out = "0" + out
        else
          out = (digit + (carry ? 1 : 0)).to_s + out
          carry = false
        end
        i = i - 1
      end
      carry ? "1" + out : out
    end

  end
end

require_relative "active_support_number_helper_mixin"
