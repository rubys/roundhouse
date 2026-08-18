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

# `compact_blank` (core_ext/enumerable.rb + core_ext/hash.rb) — the
# blank family's collection form, and the one member of it `lower::blank`
# cannot always ground.
#
# That pass rewrites `xs.compact_blank` through the ELEMENT type and
# files a `blank_unlowered` diagnostic when the element is untyped,
# saying in as many words that "the CRuby overlay serves it at
# runtime". It did not: nothing here defined the method, so the ledger
# entry promised a fallback that raised NoMethodError. campfire's
# `sessions.pluck(:ip_address).compact_blank` (a concern body, where
# `self` is not yet typed) is the site that showed it.
#
# Enumerable, not Array: Rails defines it there, so a Hash keeps its
# own pair-shaped override below and every other collection gets the
# element form.
module Enumerable
  def compact_blank
    reject(&:blank?)
  end
end

class Hash
  # Rails rejects on the VALUE and keeps the key — `{ host: nil,
  # protocol: "https" }.compact_blank` is `{ protocol: "https" }`,
  # which is how campfire's `SetCurrentRequest` builds default_url_options.
  def compact_blank
    reject { |_k, v| v.blank? }
  end
end

# A string an app has explicitly marked html-safe (`raw(...)`,
# `.html_safe`) — the overlay's ActiveSupport::SafeBuffer equivalent.
# The only behavior it adds is `html_safe?` returning true, which the
# overlay's escape-aware `html_escape` honors (see
# action_view_safe_buffer.rb). Escaping is still decided positionally
# at emit time wherever the walker can see the producer; SafeString
# carries safety across the value boundaries it can't — a `raw()`
# label threaded through an app helper's parameter into `link_to`, or
# a model method that ends in `.html_safe` (lobsters' Hat#to_html_label).
class SafeString < String
  def html_safe?
    true
  end

  def html_safe
    self
  end

  # String#to_s on a subclass returns a plain String copy; safety must
  # survive the `.to_s` coercions the emit sprinkles on helper args.
  def to_s
    self
  end
end

class String
  # AS specializes String#blank? to treat whitespace-only as blank.
  def blank?
    empty? || match?(/\A[[:space:]]*\z/)
  end

  # `html_safe` promotes to the marked type; plain strings answer
  # `html_safe?` false so the escape-aware `html_escape` escapes them.
  # (Both were identity/true in the earlier positional-only world —
  # nothing consumed `html_safe?` then; the SafeString-aware
  # html_escape override is its first consumer.)
  def html_safe
    SafeString.new(self)
  end

  def html_safe?
    false
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
    # Default-separator fast path: literal regexes are compiled once and
    # cached per call-site, where the general path below rebuilds two
    # regexes from `Regexp.escape(separator)` on every call. `separator`
    # is "-" for essentially every caller (Story#title_as_url et al.),
    # so this halves both the time and the allocations of the hottest
    # parameterize sites while staying byte-identical.
    if separator == "-"
      return downcase
        .gsub(/[^a-z0-9\-_]+/, "-")
        .gsub(/-{2,}/, "-")
        .gsub(/\A-|-\z/, "")
    end
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

  # AS's ordinal readers (core_ext/array/access.rb). Rails ships
  # `second` through `fifth` plus `forty_two`; the corpus reaches
  # `third` (campfire's `messages_controller_test` pages relative to
  # `@messages.third`), and the neighbours come with it because they
  # are the same one-liner and a list that stops at the first caller is
  # the shape that drifts.
  #
  # `forty_two` is deliberately NOT here: it is a joke method, and a
  # reader who meets it in this file should find it in Rails' docs
  # rather than in ours.
  def second
    self[1]
  end

  def third
    self[2]
  end

  def fourth
    self[3]
  end

  def fifth
    self[4]
  end

  # AS `without` / `excluding` (core_ext/enumerable.rb): the receiver
  # minus the named elements. `Relation` already answers both (they are
  # what `Room.opens.without(current)` needs); an Array did not, and a
  # test body holding a materialized association is exactly where the
  # Array form turns up — campfire's `rooms/refreshes_controller_test`
  # writes `@room.messages.without(@old_message)`.
  #
  # Rails compares with `==`; two loads of the same row are different
  # objects here, so this is IDENTITY on a model unless the model
  # defines `==`. Recorded rather than worked around: `Base` documents
  # why it defines no equality protocol.
  def without(*values)
    reject { |x| values.include?(x) }
  end

  alias_method :excluding, :without
end
