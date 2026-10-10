# Pure string ActiveSupport helpers for the Ruby/Spinel scaffold.
# `active_support_ext.rb` keeps blank?/present?/… (`is_a?` dispatch
# that AOT targets cannot host) and requires this file so scaffold
# trees share one ActiveSupport module. Not yet in strict-target
# runtime_loader tables — their string emit does not host the
# char-walk bodies; controller_name/path are literalized instead.
module ActiveSupport
  # Not ActiveSupport's regex pipeline: without its acronym and human
  # tables the steps it runs are these string walks.
  def self.underscore(text)
    s = text.to_s.split("::").join("/")
    out = +""
    n = s.length
    i = 0
    while i < n
      c = s[i].to_s
      if c >= "A" && c <= "Z" && i > 0
        prev = s[i - 1].to_s
        nxt = i + 1 < n ? s[i + 1].to_s : ""
        prev_lower = (prev >= "a" && prev <= "z") || (prev >= "0" && prev <= "9")
        prev_upper = prev >= "A" && prev <= "Z"
        next_lower = nxt >= "a" && nxt <= "z"
        out << "_" if prev_lower || (prev_upper && next_lower)
      end
      out << (c == "-" ? "_" : c)
      i = i + 1
    end
    out.downcase
  end

  # `String#remove(*patterns)` — literal substring delete, not a Regexp
  # rewrite. Patterns that are not strings are stringified once.
  def self.remove(text, pattern)
    text.to_s.split(pattern.to_s).join("")
  end

  def self.demodulize(text)
    s = text.to_s
    idx = s.rindex("::")
    idx ? s[(idx + 2)..-1].to_s : s
  end

  # Regular-suffix singularize only. Analyze folds `String#singularize`
  # into an ivar name only when this answer matches `naming::singularize`
  # (irregular / uncountable tables); otherwise the ivar stays unresolved.
  def self.singularize(text)
    s = text.to_s
    return s if s.empty?
    return s[0, s.length - 3].to_s + "y" if s.end_with?("ies") && s.length > 3
    return s[0, s.length - 2].to_s if s.end_with?("ses") || s.end_with?("xes") || s.end_with?("zes") || s.end_with?("ches") || s.end_with?("shes")
    return s[0, s.length - 1].to_s if s.end_with?("s") && !s.end_with?("ss")
    s
  end

  def self.humanize(text)
    raise NoMethodError, "undefined method 'humanize' for nil" if text.nil?
    s = text.to_s.tr("_", " ").lstrip
    s = s[0, s.length - 3].to_s if s.end_with?(" id")
    s = s.downcase
    return s if s.empty?
    s[0].to_s.upcase + s[1, s.length - 1].to_s
  end

  def self.titleize(text)
    raise NoMethodError, "undefined method 'titleize' for nil" if text.nil?
    s = humanize(underscore(text))
    out = +""
    n = s.length
    i = 0
    while i < n
      c = s[i].to_s
      prev = i > 0 ? s[i - 1].to_s : ""
      word_prev = (prev >= "a" && prev <= "z") || (prev >= "A" && prev <= "Z") || (prev >= "0" && prev <= "9") || prev == "_"
      out << (c >= "a" && c <= "z" && !word_prev ? c.upcase : c)
      i = i + 1
    end
    out
  end
end

# Rails 8.1.4 Inflector methods that do not depend on pluralization,
# transliteration, constant lookup, or locale-specific rule catalogs. String
# methods match default ASCII identifier behavior; configured acronym tables
# are intentionally not modeled. `classify`/`tableize`, locale mutation,
# constantization, transliteration, and parameterization remain unexposed.
module ActiveSupport
  module Inflector
    # Rails' camelize with the default (upper camel case) and lower-camel
    # option. A slash separates namespaces; an underscore starts a new word.
    def self.camelize(text, uppercase_first_letter = true)
      s = text.to_s
      out = +""
      n = s.length
      i = 0
      capitalize = uppercase_first_letter
      normalize_word = false

      if s.start_with?("::")
        out << "::"
        i = 2
        capitalize = false
      elsif s.start_with?("/")
        out << "::"
        i = 1
        capitalize = true
        normalize_word = true
      end

      while i < n
        c = s[i].to_s
        if c == "/"
          out << "::"
          capitalize = true
          normalize_word = true
        elsif c == "_"
          capitalize = true
          normalize_word = true
        else
          ascii_word_character = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || (c >= "0" && c <= "9")
          normalize_word = false if normalize_word && !ascii_word_character
          if i == 0 && !uppercase_first_letter
            out << c.downcase
          else
            out << (capitalize ? c.upcase : (normalize_word ? c.downcase : c))
          end
          capitalize = false
        end
        i = i + 1
      end
      out
    end

    # Rails' String-based namespace removal; this does not resolve constants.
    def self.deconstantize(text)
      s = text.to_s
      idx = s.rindex("::")
      idx ? s[0, idx].to_s : ""
    end

    # Rails assumes a top-level class name and removes its namespace before
    # underscoring. The optional separator flag is significant to callers.
    def self.foreign_key(class_name, separate_class_name_and_id_with_underscore = true)
      base = ActiveSupport.underscore(ActiveSupport.demodulize(class_name))
      base + (separate_class_name_and_id_with_underscore ? "_id" : "id")
    end

    # Uppercase only the first character; preserve the remainder unchanged.
    def self.upcase_first(text)
      s = text.to_s
      return s if s.empty?
      s[0].to_s.upcase + s[1, s.length - 1].to_s
    end

    # Lowercase only the first character; preserve the remainder unchanged.
    def self.downcase_first(text)
      s = text.to_s
      return s if s.empty?
      s[0].to_s.downcase + s[1, s.length - 1].to_s
    end

    # Rails' default integer ordinal suffix rules. Configuration and
    # locale-specific ActiveSupport behavior do not affect this slice.
    def self.ordinalize(number)
      magnitude = number.abs
      remainder_hundred = magnitude % 100
      suffix = "th"
      if remainder_hundred < 11 || remainder_hundred > 13
        remainder_ten = magnitude % 10
        if remainder_ten == 1
          suffix = "st"
        elsif remainder_ten == 2
          suffix = "nd"
        elsif remainder_ten == 3
          suffix = "rd"
        end
      end
      number.to_s + suffix
    end
  end
end
