# CRuby-only ActiveSupport core extensions (Object / String / Array).
#
# The reopened-builtin surface the lobsters corpus actually calls —
# ActiveSupport itself ships these as core_ext monkey-patches, so the
# CRuby overlay mirrors that shape. Reopening builtins is overlay-only
# (the shared transpiled runtime can't); strict targets get typed
# lowerings for these when lobsters comes up on them.
#
# Inflection here is the rule subset the corpus words exercise
# (story→stories, category→categories, comment→comments, …), not
# ActiveSupport's full irregular/uncountable tables.

class Object
  # `blank?`/`present?` (core_ext/object/blank.rb). The generic form
  # covers nil (!nil → true), false, numerics, and containers via
  # `empty?`. lobsters calls `present?` ~113 times.
  def blank?
    respond_to?(:empty?) ? !!empty? : !self
  end

  def present?
    !blank?
  end

  # `presence` (core_ext/object/blank.rb): the receiver when present,
  # else nil — the `url.presence || fallback` idiom.
  def presence
    present? ? self : nil
  end

  # `to_param` (core_ext/object/to_param): the URL-segment form of a
  # value, `to_s` by default. Models with a custom `to_param` (lobsters'
  # Domain routes on its name) override this; generated route helpers
  # call it on every segment arg.
  def to_param
    to_s
  end
end

class String
  # AS specializes String#blank? to treat whitespace-only as blank.
  def blank?
    empty? || match?(/\A[[:space:]]*\z/)
  end

  # `html_safe` — identity in this string world. There is no SafeBuffer:
  # escaping is decided positionally (the view walker escapes bare
  # interpolations and leaves tag-producing/helper calls raw), so a
  # string an app marks safe is simply itself. `html_safe?` answers true
  # for the same reason — by the time a string is asked, positional
  # escaping has already happened.
  def html_safe
    self
  end

  def html_safe?
    true
  end

  # `'story'.pluralize(count)` — singular when count == 1, else the
  # plural form. Every corpus call site passes a count; the inflection
  # applies to the string's tail, matching AS ("#{n} story".pluralize(n)
  # → "2 stories"). A trailing "s" is treated as already-plural
  # (idempotent, like AS's /s$/ → "s" rule).
  def pluralize(count = nil)
    return self if count == 1
    if match?(/([^aeiouy]|qu)y\z/i)
      sub(/y\z/i, "ies")
    elsif end_with?("s")
      self
    elsif match?(/(x|z|ch|sh)\z/i)
      "#{self}es"
    else
      "#{self}s"
    end
  end

  def singularize
    if match?(/([^aeiouy]|qu)ies\z/i)
      sub(/ies\z/i, "y")
    elsif match?(/(s|x|z|ch|sh)es\z/i)
      sub(/es\z/i, "")
    elsif end_with?("s") && !end_with?("ss")
      sub(/s\z/i, "")
    else
      self
    end
  end

  # AS `parameterize`: downcase, squash non-alphanumerics to the
  # separator, collapse runs, trim edges. (No transliteration pass —
  # the corpus titles are ASCII; non-ASCII chars drop like Rails'
  # post-transliterate "?" placeholders do under gsub.)
  def parameterize(separator: "-")
    downcase
      .gsub(/[^a-z0-9\-_]+/, separator)
      .gsub(/#{Regexp.escape(separator)}{2,}/, separator)
      .gsub(/\A#{Regexp.escape(separator)}|#{Regexp.escape(separator)}\z/, "")
  end
end

class Array
  # AS `to_sentence`, default :en connectors: "a", "a and b",
  # "a, b, and c".
  def to_sentence
    case length
    when 0 then ""
    when 1 then self[0].to_s
    when 2 then "#{self[0]} and #{self[1]}"
    else "#{self[0..-2].join(', ')}, and #{self[-1]}"
    end
  end
end

# AS Hash#reverse_merge — self's entries win over the defaults
# (lobsters' ApplicationHelper#link_post option defaults).
class Hash
  def reverse_merge(other)
    other.merge(self)
  end

  def reverse_merge!(other)
    replace(other.merge(self))
  end
end
