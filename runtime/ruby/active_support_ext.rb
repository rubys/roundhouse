# frozen_string_literal: true
# ActiveSupport blank-predicate reopen — ruby-family surface only
# (inflector_ext.rb pattern): shipped to the scaffold trees via the
# project.rs stems list, NOT in the runtime_loader tables, so the
# strict-target transpilers never see the `is_a?` dispatch below.
#
# String inflection helpers (`underscore` / `demodulize` / …) live in
# `active_support_inflections.rb` for the Ruby/Spinel scaffold. Strict
# targets do not host that char-walk yet — controller_name/path are
# AOT-literalized in the controller lowerer instead. Require it here
# so the scaffold ActiveSupport module is one constant with both
# surfaces.
require_relative "active_support_inflections"
require_relative "hash_deep_merge"
require_relative "active_support_number_helper"
#
# `src/lower/blank.rs` grounds `blank?`/`present?`/`presence` by the
# receiver's static type and every target compiles the result. What it
# CANNOT ground is a receiver it has no type for — an untyped reader, an
# unresolved inference var, a multi-variant union. Those kept their
# dynamic `x.present?` call, which only CRuby could serve (the core_ext
# reopen of Object); on an AOT tree the send had nowhere to land, and
# lobsters' `Pushover.API_TOKEN.present?` — an unassigned `cattr_accessor`
# read, i.e. nil — took every /settings render down with it.
#
# So the residue routes HERE instead. Taking the receiver as an ARGUMENT
# is what makes this legal where the type-directed forms are not: the
# value is evaluated exactly once, so a receiver with effects grounds
# too, and no `respond_to?` is needed because the branch is on the value.
#
# Semantics are Rails', not an approximation — nil, false, and an empty
# String/Array/Hash are blank; `0` and `:sym` are present. Verified shape
# by shape against `Object#blank?`.
#
# The STRING tail is `strip.empty?`, not `empty?`: ActiveSupport's
# `String#blank?` is `/\A[[:space:]]*\z/`, so `" ".blank?` is true.
# `lower::blank` grounds the typed String sites the same way and for the
# same reason — the two must agree, because which of them serves a given
# call is a fact about type INFERENCE, not about the program. campfire's
# bot API is what priced it: `raw_request_body.blank?` reaches this
# function (the reader's type is not inferred), and a boost posted with
# a body of three spaces was accepted where Rails answers 422.
#
# `to_s` first, so the tail is one branch for every remaining shape: an
# Integer's `"0"` is not blank, a Symbol's `"sym"` is not blank, and a
# String is itself.
module ActiveSupport
  # `ActiveSupport::JSON` — Rails' JSON coder over the json library.
  #
  # `encode` is `value.as_json.to_json` with Rails' default HTML-entity
  # escaping (`JsonBuilder.escape_html_entities`; the U+2028/U+2029
  # separators are left alone, as `load_defaults` 8.1+ leaves them).
  # campfire encodes a flat Hash for the unread notice
  # (`ActiveSupport::JSON.encode(roomId: room.id)`, broadcast with
  # `coder: nil`, basecamp/once-campfire#292), and since its response
  # cache an Array of request facts (`CachedResponses#response_cache_key`)
  # and the raw attributes of cached records (`RecordCache.fetch`).
  #
  # `as_json` is spelled out for the values those carry — nil, booleans,
  # numbers, Strings, Symbols, Times, and Arrays/Hashes of them, keys as
  # their `to_s`, a non-finite Float as null, as Rails answers. Anything
  # else (a record) raises rather than encode some other text.
  #
  # `decode` is `JSON.parse`, which is all Rails' is while
  # `ActiveSupport.parse_json_times` keeps its default (off).
  module JSON
    def self.encode(value)
      JsonBuilder.escape_html_entities(::JSON.generate(ActiveSupport::JSON.jsonable(value)))
    end

    # `encode(value, escape: false)`, which `lower::symbolize_keys`
    # routes here: the same document without the HTML-entity escaping,
    # for text no page embeds (a digest's input).
    def self.encode_without_escape(value)
      ::JSON.generate(ActiveSupport::JSON.jsonable(value))
    end

    def self.decode(text)
      ::JSON.parse(text)
    end

    # One `case` rather than a chain of tests: each read of the untyped
    # value is a site the runtime typing gate counts.
    def self.jsonable(value)
      case value
      when Array then value.map { |item| ActiveSupport::JSON.jsonable(item) }
      when Hash
        out = {}
        value.each { |key, item| out[key.to_s] = ActiveSupport::JSON.jsonable(item) }
        out
      when Float then value.finite? ? value : nil
      when Symbol then value.to_s
      when Time then ActiveSupport::JSON.time_json(value)
      when nil, true, false, Integer, String then value
      else raise ArgumentError, "ActiveSupport::JSON.encode: #{value.class} values are not supported yet"
      end
    end

    # A Time as Rails' `TimeWithZone#as_json` writes it: `xmlschema(3)`,
    # three fractional digits (truncated, as xmlschema does) and the
    # offset, with `Z` for UTC. The runtime's Times stand in for
    # TimeWithZone values in the app's zone, and a zone with no offset is
    # taken to be UTC (Rails writes `+00:00` for a non-UTC zone at offset
    # zero, such as London in winter).
    def self.time_json(t)
      # Concatenate onto a fresh local: Spinel refuses a shared String
      # written from a conditional into the format append below.
      zone = (t.zone == "UTC" || (t.utc_offset == 0 && ActiveSupport::JSON.utc_zone?) ? "Z" : t.strftime("%:z")) + ""
      t.strftime("%Y-%m-%dT%H:%M:%S") + format(".%03d", t.usec / 1000) + zone
    end

    # Not inlined at the call site: a strict target has built no written-down encoder, and this ruby-family/spinel seam is what reports it unsupported there.
    def self.written(text)
      text
    end

    # Not offset zero alone: Rails writes `Z` only when the zone is UTC itself, and `+00:00` for London in winter.
    def self.utc_zone?
      zone = ActiveSupport.current_zone
      name = zone.nil? ? Rails.application.config_time_zone : zone.name
      utc_zone_name?(name)
    end

    def self.utc_zone_name?(name)
      UTC_ZONE_NAMES.include?(name)
    end

    UTC_ZONE_NAMES = ["UTC", "Etc/UTC", "Universal", "Etc/Universal", "Zulu", "Etc/Zulu", "UCT", "Etc/UCT"].freeze

    # Not `time_json` at the call site: a nil Time is `null`, and a written-down encoder would otherwise read its value twice.
    def self.time_fragment(t)
      return "null" if t.nil?
      "\"" + ActiveSupport::JSON.time_json(t) + "\""
    end
  end

  # ActiveSupport's `deep_dup` (core_ext/object/deep_dup.rb). One walk,
  # also what `ErrorReporter` hands each subscriber. A Hash is copied
  # into a fresh Hash: String and Symbol keys first, every other key
  # after them, which is where Rails' delete-and-store lands. An Array
  # copies each element. A Method, an UnboundMethod, a Singleton, or
  # anything that answers `duplicable?` with false is itself; anything
  # else is its `dup` (an Integer's, nil's or a Symbol's is itself).
  # `lower::symbolize_keys` routes a typed Hash or Array receiver here.
  def self.deep_dup(value)
    case value
    when Array then value.map { |item| ActiveSupport.deep_dup(item) }
    when Hash
      # A fresh Hash, not `dup`: String and Symbol keys first, every
      # other key after them. The key class is read once; the second
      # pass used to ask it again.
      out = {}
      deferred_keys = []
      deferred_items = []
      value.each do |key, item|
        if key.is_a?(String) || key.is_a?(Symbol)
          out[key] = ActiveSupport.deep_dup(item)
        else
          deferred_keys << key
          deferred_items << item
        end
      end
      i = 0
      while i < deferred_keys.length
        out[ActiveSupport.deep_dup(deferred_keys[i])] = ActiveSupport.deep_dup(deferred_items[i])
        i += 1
      end
      out
    else
      # Not duplicable? (core_ext/object/duplicable.rb): the value itself.
      if value.is_a?(Method) || value.is_a?(UnboundMethod) || (defined?(::Singleton) && value.is_a?(::Singleton))
        value
      elsif value.respond_to?(:duplicable?) && !value.duplicable?
        value
      else
        value.dup
      end
    end
  end

  def self.blank?(value)
    return true if value.nil?
    return true if value == false
    return value.empty? if value.is_a?(Array)
    return value.empty? if value.is_a?(Hash)
    value.to_s.strip.empty?
  end

  def self.present?(value)
    !blank?(value)
  end

  # ActiveSupport's `Object#to_param`, for a receiver inference could not
  # type — `lower::to_param_residue` routes it here, as `lower::blank`
  # routes an untyped `blank?`. lobsters' anonymous story-list cache key
  # is the shape:
  #
  #     opts.merge(page: page).sort.map { |k, v| "#{k}=#{v.to_param}" }
  #
  # with `true`, an Integer and the `length` Hash as the values. Rails'
  # answers by kind: `to_s` for true/false/numbers/Symbols, the String
  # itself, `to_query` for a Hash, the elements' params joined by "/" for
  # an Array. A record keeps its own `to_param` (Story's is `short_id`),
  # so that one stays a send and dispatches on the model.
  #
  # `nil.to_param` is nil in Rails; the lowering only routes sites whose
  # result is interpolated, where nil renders "", so "" is what this
  # answers and the method stays String-typed.
  #
  # `to_s` IS Rails' answer for every scalar — nil (""), true/false,
  # numbers, Symbols, Strings — so only the containers and records
  # branch. Fewer reads of the untyped parameter is fewer untyped sites
  # (the runtime's typing ceiling counts each one).
  def self.to_param(value)
    return ActionView::ViewHelpers.to_query(value) if value.is_a?(Hash)
    return value.map { |e| ActiveSupport.to_param(e) }.join("/") if value.is_a?(Array)
    return value.to_param.to_s if value.is_a?(ActiveRecord::Base)
    value.to_s
  end

  # `Hash#to_query` (no namespace), for a receiver `lower::symbolize_keys`
  # routes here: a Hash whose values may nest, or one inference could not
  # type (`request.query_parameters.deep_dup.to_query`). It is ONE
  # grammar on every target, activesupport 8.1's
  # `core_ext/object/to_query.rb` rule for rule: `key=value`, both
  # CGI-escaped; a nested Hash is `parent[child]=…`; an Array is
  # `key[]=…` per element; nil is the bare escaped key; an empty Hash or
  # Array VALUE is dropped. The `"k=v"` strings of a level are SORTED —
  # the encoded pairs, not the keys — except under an Array namespace
  # (a `[]` in it), where order is the array's. The result is a 301
  # target in EngineeredAt, so it must be byte-identical to Rails'.
  #
  # Not `ViewHelpers.to_query`: that is the SHARED scalar rendering in
  # insertion order, and the nesting version is a ruby-family reopen. A
  # receiver that is not a Hash raises, as Rails' `Object#to_query`
  # does for want of its `key` argument.
  def self.to_query(value)
    raise ArgumentError, "wrong number of arguments (given 0, expected 1)" unless value.is_a?(Hash)
    query_pairs(value, "")
  end

  def self.query_pairs(hash, namespace)
    pairs = []
    hash.each do |key, item|
      empty_container = (item.is_a?(Hash) && item.empty?) || (item.is_a?(Array) && item.empty?)
      next if empty_container
      # One `to_s` of the untyped key. The ceiling counts each read.
      key_text = key.to_s
      name = namespace.empty? ? key_text : "#{namespace}[#{key_text}]"
      pairs << query_value(name, item)
    end
    pairs = pairs.sort unless namespace.include?("[]")
    pairs.join("&")
  end

  # `CGI.escape`: BYTE-wise, so a non-ASCII character becomes its UTF-8
  # `%XX` run (`é` -> `%C3%A9`) and a control character its `%09`.
  # Unreserved is `A-Za-z0-9 _ . - ~`; a space is `+`. The shared
  # `ViewHelpers.url_encode` is a table over the ASCII punctuation and
  # passes every other byte through, which is fine for a path segment
  # a router reads back and wrong for a 301 target.
  def self.query_escape(text)
    hex = "0123456789ABCDEF"
    out = +""
    i = 0
    n = text.bytesize
    while i < n
      b = text.getbyte(i)
      if (b >= 48 && b <= 57) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122) || b == 45 || b == 46 || b == 95 || b == 126
        out << b.chr
      elsif b == 32
        out << "+"
      else
        out << "%" << hex[b / 16] << hex[b % 16]
      end
      i += 1
    end
    out
  end

  def self.query_value(name, item)
    if item.is_a?(Hash)
      query_pairs(item, name)
    elsif item.is_a?(Array)
      prefix = "#{name}[]"
      return query_escape(prefix) if item.empty?
      parts = []
      item.each { |element| parts << query_value(prefix, element) }
      parts.join("&")
    elsif item.nil?
      query_escape(name)
    else
      "#{query_escape(name)}=#{query_escape(item.to_s)}"
    end
  end

  def self.presence(value)
    blank?(value) ? nil : value
  end

  # Rails' `Object#presence_in(another)` — `in?(another) ? self : nil`,
  # the allow-list spelling for a value that came off the wire
  # (campfire's `params.require(:user)[:role].presence_in(%w[ member
  # administrator ])`). Grounded here rather than as a core_ext reopen
  # so every target has a method to dispatch; the list is a String
  # allow-list in every corpus site, which is what lets the parameter
  # be declared rather than left untyped.
  def self.presence_in(value, list)
    value.is_a?(String) && list.include?(value) ? value : nil
  end

  # Rails' `ActiveModel::Errors#to_hash`: each attribute with errors, in
  # the order of its first error, to its messages
  # (`{ title: ["can't be blank"] }`). Reached from
  # `src/lower/errors_index.rs` with the record's `error_details`. Takes
  # the details as an ARGUMENT, not a `Base` method: every strict target
  # transpiles `Base`, so a method there would need an implementation and
  # a check on each of them.
  def self.errors_to_hash(details)
    out = {}
    details.each do |detail|
      key = detail.attribute.to_sym
      out[key] = [] unless out.key?(key)
      out[key] << detail.message
    end
    out
  end

  # Not `errors_to_hash` itself: Rails' `messages` answers `[]` for an attribute with no errors, and is frozen.
  def self.errors_messages(details)
    out = errors_to_hash(details)
    out.default = [].freeze
    out.freeze
  end

  def self.errors_attribute_names(details)
    out = []
    details.each do |detail|
      key = detail.attribute.to_sym
      out << key unless out.include?(key)
    end
    out.freeze
  end

  # Not a `reject!` on each: `errors` holds each detail's full message index for index, so both lose the same entries.
  def self.errors_delete(errors, details, attribute)
    deleted = []
    i = details.length - 1
    while i >= 0
      if details[i].attribute == attribute
        deleted.unshift(details[i].message)
        details.delete_at(i)
        errors.delete_at(i)
      end
      i -= 1
    end
    deleted.empty? ? nil : deleted
  end

  # Rails' `details`: each attribute's errors, in the order of its first
  # one. `lower::errors_index` reads an entry's `[:error]` as the type.
  def self.errors_details(details)
    out = {}
    details.each do |detail|
      key = detail.attribute.to_sym
      out[key] = [] unless out.key?(key)
      out[key] << detail
    end
    out.default = [].freeze
    out.freeze
  end

  # Rails' `errors.import(error, attribute:)`: the error under
  # `attribute` (its own when nil), its full message named as the
  # importing model names that attribute. `names` is the importing
  # model's translated attribute names, folded from the locale; any other
  # is humanized, as `human_attribute_name` falls back to.
  def self.errors_import(errors, details, error, attribute, names, before, between, after)
    name = attribute.nil? ? error.attribute : attribute
    full = error.message
    unless name == "base"
      human = names.key?(name) ? names[name].to_s : humanize(name.tr(".", "_"))
      full = before + human + between + error.message + after
    end
    errors << full
    details << ActiveRecord::ErrorDetail.new(name, error.kind, error.message, full, error.symbolic, error.optionless)
    details
  end

  # Rails' `errors.add(attribute, kind_or_message, **options)` where the
  # attribute or the options are known only at run time (a validator's
  # `validate_each`). The message is looked up as Rails does: the model's
  # own (`overrides`, "<attr>.<kind>" then "<kind>", with its "__scope"),
  # then what the call site baked for its literal kind (`site`):
  # "ar_messages" or "am_messages" (<scope>.errors.messages.<kind>),
  # "attributes.<attr>" (errors.attributes.<attr>.<kind>) and "messages"
  # (errors.messages.<kind>), each with ".one" / ".other" / ".zero" plural
  # forms. A String `message` is used as given. The attribute is named by
  # the model's translated names (`names`), else humanized. Answers the
  # error's detail; the call site pushes it and its full message onto the
  # record's errors.
  def self.error_detail(name, kind, message, options, site, names, overrides, before, between, after)
    count = options["count"]
    if kind.nil?
      template = message.to_s
    else
      scoped = overrides["__scope"] == "activemodel" ? "am_messages" : "ar_messages"
      template = error_template(overrides, name + "." + kind, count) ||
        error_template(overrides, kind, count) ||
        error_template(site, scoped, count) ||
        error_template(site, "attributes." + name, count) ||
        error_template(site, "messages", count) ||
        ("translation missing: errors.messages." + kind)
    end
    human = names.key?(name) ? names[name].to_s : humanize(name.tr(".", "_"))
    text = template
    # A String message is kept as written, `%{…}` and all. A kind's
    # template takes Rails' default interpolations, an explicit option
    # winning.
    unless kind.nil?
      values = { "attribute" => human, "model" => names.key?("__model") ? names["__model"].to_s : "" }
      options.each { |key, value| values[key] = value }
      text = interpolate(template, values)
    end
    full = text
    unless name == "base"
      full = before + human + between + text + after
    end
    ActiveRecord::ErrorDetail.new(name, kind.nil? ? text : kind, text, full, !kind.nil?, options.empty?)
  end

  # I18n's interpolation, in one pass as its `gsub` makes: each `%{name}`
  # the values give is replaced by its value as written (not rescanned,
  # and not read for `\0` or `\&`), `%%` is a literal `%`, and any other
  # `%{name}` stays as written.
  def self.interpolate(template, values)
    out = ""
    rest = template
    scanning = true
    while scanning
      at = rest.index("%")
      if at.nil? || at + 1 >= rest.length
        scanning = false
      else
        out = out + rest[0, at].to_s
        after = rest[at + 1, 1].to_s
        close = after == "{" ? rest.index("}", at + 2) : nil
        if after == "%"
          out = out + "%"
          rest = rest[at + 2, rest.length - at - 2].to_s
        elsif !close.nil? && values.key?(rest[at + 2, close - at - 2].to_s)
          out = out + values[rest[at + 2, close - at - 2].to_s].to_s
          rest = rest[close + 1, rest.length - close - 1].to_s
        else
          out = out + "%"
          rest = rest[at + 1, rest.length - at - 1].to_s
        end
      end
    end
    out + rest
  end

  # The template under `key` in `table`, its plural form chosen by
  # `count` as English does (zero, one, other), else the plain one.
  def self.error_template(table, key, count)
    unless count.nil?
      form = count.to_s == "1" ? "one" : (count.to_s == "0" && table.key?(key + ".zero") ? "zero" : "other")
      return table[key + "." + form] if table.key?(key + "." + form)
    end
    table[key]
  end

  # Rails' `errors.merge!(other)`: each of the other record's errors
  # imported under its own attribute, and nothing when it is these errors.
  def self.errors_merge(errors, details, others, names, before, between, after)
    return details if others.equal?(details)
    others.each { |error| errors_import(errors, details, error, nil, names, before, between, after) }
    others
  end

  # `ActiveSupport::MessageVerifier::InvalidSignature` — what a signed
  # value that does not verify raises. Rails hangs it off the verifier
  # class, and the NAME is the whole point: a controller that rescues it
  # is naming this class, so answering some other error means the rescue
  # never fires. campfire's `Users::AvatarsController` is
  # `rescue_from(ActiveSupport::MessageVerifier::InvalidSignature) {
  # head :not_found }` over an avatar URL that carries a signed id.
  #
  # A namespace with no verifier in it, because ours is
  # `ActionController::MessageVerifier` (it serves the cookie jar first).
  # The error keeps Rails' path so app code that names it resolves.
  class MessageVerifier
    class InvalidSignature < StandardError
      def initialize(message = "ActiveSupport::MessageVerifier::InvalidSignature")
        super(message)
      end
    end
  end

  # `list.index_by { |x| key }` — the collection as a Hash keyed by the
  # block's value, last write winning on duplicates (Rails' contract).
  #
  # ActiveSupport ships this as an `Enumerable` reopen, which only the
  # CRuby overlay can host. Grounded here by `lower::enumerable_ext`,
  # which takes the receiver as an ARGUMENT for the same reason
  # `blank?` above does: the collection is evaluated exactly once.
  #
  # The `ActiveRecord::Relation` twin (relation.rb) stays where it is —
  # it has a real RBS signature, and routing a typed receiver through
  # this untyped one would trade a typed call for an untyped one to fix
  # nothing. Same body shape as that twin, deliberately: `h[yield x] =
  # x` inside an `each` is the form every target's emitter already
  # compiles.
  def self.index_by(list)
    h = {}
    list.each { |x| h[yield x] = x }
    h
  end

  # Rails' `Array#excluding` (`Array#without` is its alias): Array#-
  # preserves the receiver's order and any non-excluded duplicates.
  # Rails flattens one level of the excluded arguments before
  # subtracting them.
  def self.excluding(list, *elements)
    list - elements.flatten(1)
  end

  # AS `Enumerable#pluck` on a receiver the analyzer could not type
  # (lobsters' `value.pluck(:term)` over a parslet parse tree). Not
  # `map` for a Relation or a model class: Rails' own `pluck` selects only those columns.
  def self.pluck(list, *keys)
    return list.pluck(*keys) if list.is_a?(ActiveRecord::Relation) || list.is_a?(Class)
    return list.map { |element| keys.map { |key| element[key] } } if keys.length > 1

    list.map { |element| element[keys[0]] }
  end

  # AS `Enumerable#many?`: the materialized no-block form is a length
  # check; the block form counts matches and stops at the second hit.
  # Another core_ext reopen the transpiled runtimes cannot host — same
  # home and same rule as `index_by` above, the receiver is evaluated
  # exactly once.
  def self.many?(list)
    return list.length > 1 unless block_given?

    count = 0
    list.each do |item|
      count = count + 1 if yield item
      return true if count > 1
    end
    false
  end

  # AS `String#squish`: runs of whitespace collapsed to one space, and
  # the ends stripped. Rails writes it as
  # `gsub(/[[:space:]]+/, " ").strip` on a `String` reopen — a core_ext
  # the transpiled runtimes cannot host, and a REGEX the targets do not
  # all lower, so the scan is spelled out the way `to_sentence` below
  # is. Match each character against Rails' POSIX whitespace class,
  # including Unicode whitespace. The walk is over `chars`, not
  # `text[i]`: on a multibyte String CRuby finds the i-th character from
  # the start, so indexing the String made the scan quadratic. The
  # result grows in place for the same reason.
  #
  # campfire's `content_filters_test` writes `<<~HTML.squish` to put a
  # multi-line fixture body on one line before handing it to a filter.
  def self.squish(text)
    out = +""
    pending_space = false
    chars = text.chars
    n = chars.length
    i = 0
    while i < n
      c = chars[i]
      if c.match?(/[[:space:]]/)
        pending_space = !out.empty?
      else
        out << " " if pending_space
        pending_space = false
        out << c
      end
      i = i + 1
    end
    out
  end

  # AS `String#first(limit = 1)` / `String#last(limit = 1)`
  # (core_ext/string/access.rb): the leading or trailing `limit`
  # characters, the whole String when it is shorter, and ArgumentError
  # for a negative limit. A String reopen the transpiled runtimes cannot
  # host, as `squish` below. Campfire binds its transfer link to
  # `password_salt&.last(10)`.
  def self.string_first(text, limit)
    head = text[0, limit]
    raise ArgumentError, "negative limit" if head.nil?
    head
  end

  def self.string_last(text, limit)
    tail = text[[text.length - limit, 0].max, limit]
    raise ArgumentError, "negative limit" if tail.nil?
    tail
  end

  # Rails' destructive form always returns the same receiver, including
  # when its contents were already squished.
  def self.squish!(text)
    text.gsub!(/[[:space:]]+/, " ")
    text.strip!
    text
  end

  # AS `String#truncate(truncate_to, omission: "...", separator: nil)`
  # (core_ext/string/filters.rb), with the options positional: the
  # result is at most `truncate_to` characters including the omission,
  # unless the omission alone is longer, in which case it is the
  # omission. A separator cuts at its last occurrence that still leaves
  # room for the omission; when it does not occur there, the cut is
  # where it would have been without one. `nil` for either option means
  # what Rails' `options[:omission] || "..."` and `if
  # options[:separator]` make it mean. A String reopen the transpiled
  # runtimes cannot host - same home and same rule as `squish` above.
  def self.truncate(text, truncate_to, omission, separator)
    return text.dup unless text.length > truncate_to

    mark = omission || "..."
    room = truncate_to - mark.length
    return mark.dup if room <= 0

    stop = room
    if separator
      found = text.rindex(separator, room)
      stop = found if found
    end
    text[0, stop] + mark
  end

  # The whitespace Rails' default `truncate_words` separator (`/\s+/`)
  # splits on, as a byte: space, or \t \n \v \f \r.
  def self.word_space_byte?(b)
    b == 32 || (b >= 9 && b <= 13)
  end

  # AS `String#truncate_words(words_count, omission: "...")`
  # (core_ext/string/filters.rb) for the default separator, with the
  # omission positional. Rails matches `/\A((?>.+?\s+){n-1}.+?)\s+.*/m`:
  # a word is at least one character, so leading whitespace belongs to
  # the first word, and the cut happens only when whitespace follows the
  # n-th word. Built without a Regexp, like `squish` above. A `separator:`
  # call is not grounded here. Same home and same rule as `truncate`.
  # The scan reads bytes: every separator is ASCII and no byte of a
  # multibyte UTF-8 character is below 0x80, so the cuts are the same,
  # and `text[i]` would walk the String from the start on each read.
  def self.truncate_words(text, words_count, omission)
    mark = omission || "..."
    n = text.bytesize
    i = 0
    words = 0
    while i < n
      j = i + 1
      j += 1 while j < n && !word_space_byte?(text.getbyte(j))
      k = j
      k += 1 while k < n && word_space_byte?(text.getbyte(k))
      words += 1
      return text.byteslice(0, j) + mark if words == words_count && k > j
      i = k
    end
    text.dup
  end

  # AS `Enumerable#sole`: THE one element, and a raise for any other
  # count — Rails' `SoleItemExpectedError` with its two messages. A
  # core_ext reopen (`Enumerable`) the transpiled runtimes cannot host,
  # same home and same rule as `many?` above. campfire's
  # unread_rooms_channel_test reads `subscription.streams.sole` — the
  # stream a channel confirmed, and the assertion that it confirmed
  # exactly one, in one call; `first` would keep the read and drop the
  # assertion.
  def self.sole(list)
    n = list.length
    raise "no item found" if n == 0
    raise "multiple items found" if n > 1
    list[0]
  end

  # AS `Hash#symbolize_keys` over a String-keyed hash: a NEW hash, each
  # key interned. The core_ext reopen (`Hash`) is out of reach for the
  # same reason as the others here. lobsters reads a raw-SQL row this way
  # (`exec_query(sql).first.symbolize_keys!` in FlaggedCommenters) —
  # rows come back keyed by column name. `lower::symbolize_keys` routes
  # the String-keyed calls here; a Symbol-keyed receiver is the identity
  # and never arrives.
  def self.symbolize_keys(hash)
    out = {}
    hash.each { |k, v| out[k.to_sym] = v }
    out
  end

  def self.stringify_keys(hash)
    out = {}
    hash.each { |k, v| out[k.to_s] = v }
    out
  end

  # Not `stringify_keys`: Rails' indifferent hash converts only a Symbol key, and its Hash and Array values all the way down.
  def self.indifferent(hash)
    out = {}
    return out if hash.nil?
    hash.each { |key, item| out[ActiveSupport.indifferent_key(key)] = ActiveSupport.indifferent_value(item, false) }
    out
  end

  # Not a copy for every Array: Rails converts one written by `[]=` in place, unless it is frozen.
  def self.indifferent_value(value, in_place)
    if value.is_a?(Hash)
      ActiveSupport.indifferent(value)
    elsif value.is_a?(Array)
      items = in_place && !value.frozen? ? value : value.dup
      # Not `map!`: spinel has no in-place map on a poly Array.
      i = 0
      while i < items.length
        items[i] = ActiveSupport.indifferent_value(items[i], in_place)
        i += 1
      end
      items
    else
      value
    end
  end

  def self.indifferent_key(key)
    key.is_a?(Symbol) ? key.to_s : key
  end

  def self.indifferent_keys(keys)
    keys.map { |key| ActiveSupport.indifferent_key(key) }
  end

  def self.indifferent_to_hash(value)
    if value.is_a?(Hash)
      out = {}
      value.each { |key, item| out[key] = ActiveSupport.indifferent_to_hash(item) }
      out
    elsif value.is_a?(Array)
      value.map { |item| ActiveSupport.indifferent_to_hash(item) }
    else
      value
    end
  end

  # Not `value == false || value == 0`: ActiveModel compares its FALSE_VALUES by string too, so :off and "0" answer false.
  # Only the String "" is nil, as `cast_value`'s `value == ""`: `:""`
  # is not in FALSE_VALUES, so it is true.
  def self.cast_boolean(value)
    return nil if value.nil?
    return nil if value == ""
    text = value.to_s
    !%w[0 f F false FALSE off OFF].include?(text)
  end

  # ActiveModel::Type::String#cast_value. `true`/`false` are the frozen
  # `"t"`/`"f"`; a String is copied so a later mutation is not the
  # attribute; anything else is `to_s`.
  def self.cast_string(value)
    if value.nil?
      nil
    elsif value == true
      "t".freeze
    elsif value == false
      "f".freeze
    elsif value.is_a?(String)
      value.dup
    else
      value.to_s
    end
  end

  # ActiveModel::Type::Integer, through `Helpers::Numeric#cast`. A blank
  # string is nil. A Float that is not finite is nil (`to_i` raises
  # `FloatDomainError`). Any other string is `to_i` of its first 16
  # bytes, Rails' `byteslice(0, limit * 4)` at the default 4-byte limit.
  # A value of another class is what `value.presence` and then
  # `value.to_i rescue nil` answer: a Time, a Duration (its seconds) or another Numeric (a
  # Rational) its `to_i`, anything else nil (a Symbol, an Array, a Hash
  # and an Object have no `to_i`). Each class is named rather than asked
  # with `respond_to?`, so every branch is typed.
  def self.cast_integer(value)
    if value.nil?
      nil
    elsif value == true
      1
    elsif value == false
      0
    elsif value.is_a?(Integer)
      value
    elsif value.is_a?(Float)
      # Not `finite?`, which not every target's Float answers. NaN and
      # the infinities are the floats for which `v - v` is not 0.0.
      if value - value != 0.0
        nil
      else
        value.to_i
      end
    elsif value.is_a?(String)
      ActiveSupport.integer_from_string(value)
    elsif value.is_a?(Time)
      value.to_i
    elsif value.is_a?(ActiveSupport::Duration)
      value.to_i
    elsif value.is_a?(Numeric)
      value.to_i
    else
      nil
    end
  end

  # `Helpers::Numeric#cast`'s `value.presence` for a value that is not
  # nil, a boolean, a number or a String: `Object#blank?`, which is
  # `empty?` for the classes that answer it (`[]`, `{}`, `:""`, `Set[]`) and
  # false for any other object.
  def self.numeric_blank?(value)
    if value.is_a?(Array)
      value.empty?
    elsif value.is_a?(Hash)
      value.empty?
    elsif value.is_a?(Symbol)
      value.empty?
    elsif value.is_a?(Set)
      value.empty?
    else
      false
    end
  end

  # ActiveModel::Type::Float, through `Helpers::Numeric#cast` (a decimal
  # column is a Float here, so it casts the same, where Rails answers a
  # BigDecimal of the same value). A blank string is nil; `true`/`false`
  # are 1.0/0.0; `"Infinity"`, `"-Infinity"` and `"NaN"` are those floats
  # (`Type::Float#cast_value`); any other string is `to_f`. A value of
  # another class is nil when blank (`[]`, `{}`, `:""`), a Time or
  # another Numeric (a Rational) or a Duration its `to_f`; Rails does not rescue that
  # `to_f`, so anything else (a Symbol, an Array, a Hash, an Object)
  # raises the NoMethodError it raises there.
  def self.cast_float(value)
    if value.nil?
      nil
    elsif value == true
      1.0
    elsif value == false
      0.0
    elsif value.is_a?(Float)
      value
    elsif value.is_a?(Integer)
      value.to_f
    elsif value.is_a?(String)
      ActiveSupport.float_from_string(value)
    elsif ActiveSupport.numeric_blank?(value)
      nil
    elsif value.is_a?(Time)
      value.to_f
    elsif value.is_a?(ActiveSupport::Duration)
      value.to_f
    elsif value.is_a?(Numeric)
      value.to_f
    else
      raise NoMethodError, "undefined method 'to_f' for an instance of #{value.class}"
    end
  end

  # ActiveModel::Type::Decimal, as a Float (a decimal column is a Float
  # here). nil, booleans, numbers and Strings cast as `cast_float` does.
  # Another value is nil when blank; another Numeric (a Rational) is its
  # value; anything else is what its `to_s` reads (`cast_value(value.to_s)`),
  # so a Symbol, an Array, a Hash or an Object is 0.0 and `:"5"` is 5.0.
  def self.cast_decimal(value)
    if value.nil? || value == true || value == false || value.is_a?(Integer) || value.is_a?(Float) || value.is_a?(String)
      ActiveSupport.cast_float(value)
    elsif ActiveSupport.numeric_blank?(value)
      nil
    elsif value.is_a?(ActiveSupport::Duration)
      value.to_f
    elsif value.is_a?(Numeric)
      value.to_f
    else
      ActiveSupport.float_from_string(value.to_s)
    end
  end

  # The number ActiveModel 8.1.4's NumericalityValidator checks, or nil
  # when it is "not a number". `raw` is the value the user assigned
  # (`<attr>_before_type_cast`), nil when the attribute did not come from
  # the user, in which case the cast `value` is read (`raw_value ||
  # value`). Then `parse_as_number`: a Numeric as it is; a String of
  # digits (`/\A[+-]?\d+\z/`) its Integer; a hex prefix not a number;
  # anything else what `Kernel.Float` reads, or not a number where it
  # raises. `Kernel.Float`'s grammar is the regexp, since not every
  # target has `Float()`.
  def self.numericality_number(raw, value)
    v = raw.nil? || raw == false ? value : raw
    return nil if v.nil?
    return v if v.is_a?(Integer) || v.is_a?(Float)
    text = v.to_s
    return text.to_i if text.match?(/\A[+-]?\d+\z/)
    return nil if text.match?(/\A[+-]?0[xX]/)
    return nil unless text.match?(/\A[[:space:]]*[+-]?(?:\d+(?:_\d+)*(?:\.(?:\d+(?:_\d+)*)?)?|\.\d+(?:_\d+)*)(?:[eE][+-]?\d+(?:_\d+)*)?[[:space:]]*\z/)
    text.to_f
  end

  # The String half of `cast_float`: the blank scan `integer_from_string`
  # makes, then Rails' three named values, then `to_f`. The named values
  # are divisions rather than `Float::INFINITY`/`NAN`, which not every
  # target defines.
  def self.float_from_string(value)
    i = 0
    n = value.length
    blank = true
    while i < n
      blank = false unless value[i].match?(/[[:space:]]/)
      i = n unless blank
      i = i + 1
    end
    return nil if blank
    zero = 0.0
    return 1.0 / zero if value == "Infinity"
    return -1.0 / zero if value == "-Infinity"
    return zero / zero if value == "NaN"
    value.to_f
  end

  # `String#blank?` is `/\A[[:space:]]*\z/`, which raises ArgumentError
  # on invalid UTF-8 — the integer writer does, in `presence`, before
  # it slices. One character at a time, the same scan `squish` lowers:
  # a counter loop, no `break` or `next` (those stay a Ruby loop, and
  # TypeScript turns `next` into `return`). `value[i]` on a multibyte
  # string walks from the start, so the loop stops at the first
  # character that is not space by moving `i` to the end, then taking
  # the required `i = i + 1` step. A string that is not blank is `to_i`
  # of its first 16 bytes. The first 16 characters are enough: `to_i`
  # reads only an ASCII prefix, and 16 characters are at least 16 bytes.
  def self.integer_from_string(value)
    i = 0
    n = value.length
    blank = true
    while i < n
      blank = false unless value[i].match?(/[[:space:]]/)
      i = n unless blank
      i = i + 1
    end
    return nil if blank
    prefix = n > 16 ? value[0, 16] : value
    ActiveSupport.integer_from_bytes(prefix.bytes)
  end

  # `byteslice(0, 16).to_i`. Strict targets lower `String#bytes`, not
  # `byteslice`. For a string `blank?` accepted, the two slices' `to_i`
  # are the same integer: `to_i` reads only an ASCII prefix, and a
  # non-ASCII byte stops it. Underscores between digits count, as
  # Ruby's `to_i` does (`"1_0"` is 10, `"1__0"` is 1). `phase` 3 ignores
  # the rest of the window so the loop stays a counter (`i = i + 1`).
  def self.integer_from_bytes(raw)
    limit = raw.length
    limit = 16 if limit > 16
    i = 0
    phase = 0
    sign = 1
    n = 0
    digits = 0
    while i < limit
      b = raw[i]
      b = -1 if b.nil?
      if phase == 0
        if b == 43 || b == 45
          sign = -1 if b == 45
          phase = 2
        elsif b >= 48 && b <= 57
          n = n * 10 + (b - 48)
          digits = digits + 1
          phase = 2
        elsif b != 32 && b != 9 && b != 10 && b != 11 && b != 12 && b != 13
          phase = 3
        end
      elsif phase == 2
        if b == 95
          nxt = raw[i + 1]
          nxt = -1 if nxt.nil?
          phase = 3 if digits == 0 || nxt < 48 || nxt > 57
        elsif b >= 48 && b <= 57
          n = n * 10 + (b - 48)
          digits = digits + 1
        else
          phase = 3
        end
      end
      i = i + 1
    end
    n * sign
  end

  # AS `Array#to_sentence`: "", "a", "a and b", "a, b, and c" with the
  # :en connectors, which `lower::enumerable_ext` passes when the call
  # site names none. Another core_ext reopen (`Array`) the transpiled
  # runtimes cannot host — same home and same rule as `index_by`, the
  # receiver evaluated exactly once. campfire names a direct room by
  # its other members: `room.users.without(me).pluck(:name).to_sentence`,
  # and a group room's initials with `two_words_connector: '+'`.
  # Elements go through `to_s`, as Rails' `join` does.
  def self.to_sentence(list, words_connector, two_words_connector, last_word_connector)
    n = list.length
    return "" if n == 0
    return list[0].to_s if n == 1
    return "#{list[0]}#{two_words_connector}#{list[1]}" if n == 2
    head = +""
    i = 0
    while i < n - 1
      head = head + words_connector if i > 0
      head = head + list[i].to_s
      i = i + 1
    end
    "#{head}#{last_word_connector}#{list[n - 1]}"
  end

  # Not reopened on `Time` (no built-in reopening), and not `Time#+`: day shifts go through the civil calendar so DST cannot move the clock.
  def self.civil_days(y, m, d)
    yy = m <= 2 ? y - 1 : y
    era = yy / 400
    yoe = yy - era * 400
    doy = (153 * (m > 2 ? m - 3 : m + 9) + 2) / 5 + d - 1
    doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
    era * 146097 + doe - 719468
  end

  def self.local_on(days, hour, min, sec, nsec)
    z = days + 719468
    era = z / 146097
    doe = z - era * 146097
    yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365
    doy = doe - (365 * yoe + yoe / 4 - yoe / 100)
    mp = (5 * doy + 2) / 153
    d = doy - (153 * mp + 2) / 5 + 1
    m = mp < 10 ? mp + 3 : mp - 9
    local_time(yoe + era * 400 + (m <= 2 ? 1 : 0), m, d, hour, min, sec, nsec)
  end

  def self.days_in_month(y, m)
    m == 12 ? civil_days(y + 1, 1, 1) - civil_days(y, 12, 1) : civil_days(y, m + 1, 1) - civil_days(y, m, 1)
  end

  def self.days_since(t, n = 1)
    local_on(civil_days(t.year, t.month, t.day) + n, t.hour, t.min, t.sec, t.nsec)
  end

  def self.days_ago(t, n = 1)
    days_since(t, -n)
  end

  def self.yesterday(t)
    days_since(t, -1)
  end

  def self.tomorrow(t)
    days_since(t, 1)
  end

  def self.weeks_since(t, n = 1)
    days_since(t, 7 * n)
  end

  def self.weeks_ago(t, n = 1)
    days_since(t, -7 * n)
  end

  # Not a day shift: ActiveSupport clamps to the target month's last day (Jan 31 + 1 month is Feb 28).
  def self.months_since(t, n = 1)
    total = t.year * 12 + t.month - 1 + n
    y = total / 12
    m = total % 12 + 1
    last = days_in_month(y, m)
    local_time(y, m, t.day > last ? last : t.day, t.hour, t.min, t.sec, t.nsec)
  end

  def self.months_ago(t, n = 1)
    months_since(t, -n)
  end

  def self.years_since(t, n = 1)
    months_since(t, 12 * n)
  end

  def self.years_ago(t, n = 1)
    months_since(t, -12 * n)
  end

  def self.change_time_usec(time, usec)
    raise ArgumentError, "argument out of range" if usec >= 1_000_000
    raise ArgumentError, "argument out of range" if time.sec * 1_000_000 + usec < 0
    changed = Time.at(time.to_i, usec, :usec)
    return changed.getutc if time.utc?
    return changed.getlocal(time.utc_offset) if time.zone.to_s.empty?
    local = Time.at(time.to_i).getlocal
    if local.zone != time.zone || local.utc_offset != time.utc_offset || local.dst? != time.dst?
      raise NotImplementedError, "Time#change with a custom or changed system timezone is not supported"
    end
    changed.getlocal
  end

  def self.beginning_of_minute(t)
    local_time(t.year, t.month, t.day, t.hour, t.min, 0, 0)
  end

  def self.end_of_minute(t)
    local_time(t.year, t.month, t.day, t.hour, t.min, 59, 999_999_999)
  end

  def self.beginning_of_hour(t)
    local_time(t.year, t.month, t.day, t.hour, 0, 0, 0)
  end

  def self.end_of_hour(t)
    local_time(t.year, t.month, t.day, t.hour, 59, 59, 999_999_999)
  end

  def self.beginning_of_day(t)
    local_time(t.year, t.month, t.day, 0, 0, 0, 0)
  end

  def self.end_of_day(t)
    local_time(t.year, t.month, t.day, 23, 59, 59, 999_999_999)
  end

  def self.noon(t)
    local_time(t.year, t.month, t.day, 12, 0, 0, 0)
  end

  # Not Sunday: `Date.beginning_of_week` defaults to Monday.
  def self.beginning_of_week(t, start = 1)
    local_on(civil_days(t.year, t.month, t.day) - (t.wday + 7 - start) % 7, 0, 0, 0, 0)
  end

  def self.end_of_week(t, start = 1)
    end_of_day(days_since(beginning_of_week(t, start), 6))
  end

  def self.next_week(t)
    beginning_of_week(days_since(t, 7))
  end

  def self.prev_week(t)
    beginning_of_week(days_since(t, -7))
  end

  def self.beginning_of_month(t)
    local_time(t.year, t.month, 1, 0, 0, 0, 0)
  end

  def self.end_of_month(t)
    local_time(t.year, t.month, days_in_month(t.year, t.month), 23, 59, 59, 999_999_999)
  end

  def self.beginning_of_year(t)
    local_time(t.year, 1, 1, 0, 0, 0, 0)
  end

  def self.end_of_year(t)
    local_time(t.year, 12, 31, 23, 59, 59, 999_999_999)
  end

  def self.same_day?(a, b)
    a.year == b.year && a.month == b.month && a.day == b.day
  end

  # Not `ActiveSupport.now` read here: the caller passes it, since this file's typing sees no clock of its own.
  def self.today?(t, now)
    same_day?(t, now)
  end

  def self.yesterday?(t, now)
    same_day?(t, days_since(now, -1))
  end

  def self.tomorrow?(t, now)
    same_day?(t, days_since(now, 1))
  end

  def self.past?(t, now)
    t < now
  end

  def self.future?(t, now)
    t > now
  end

  def self.on_weekend?(t)
    t.wday == 0 || t.wday == 6
  end

  def self.on_weekday?(t)
    !on_weekend?(t)
  end

  def self.on_wday?(t, wday)
    t.wday == wday
  end

  # Rails zone names (ActiveSupport::TimeZone::MAPPING) to IANA identifiers; an IANA identifier passes through.
  ZONE_NAMES = {
    "International Date Line West" => "Etc/GMT+12",
    "Midway Island" => "Pacific/Midway",
    "American Samoa" => "Pacific/Pago_Pago",
    "Hawaii" => "Pacific/Honolulu",
    "Alaska" => "America/Juneau",
    "Pacific Time (US & Canada)" => "America/Los_Angeles",
    "Tijuana" => "America/Tijuana",
    "Mountain Time (US & Canada)" => "America/Denver",
    "Arizona" => "America/Phoenix",
    "Chihuahua" => "America/Chihuahua",
    "Mazatlan" => "America/Mazatlan",
    "Central Time (US & Canada)" => "America/Chicago",
    "Saskatchewan" => "America/Regina",
    "Guadalajara" => "America/Mexico_City",
    "Mexico City" => "America/Mexico_City",
    "Monterrey" => "America/Monterrey",
    "Central America" => "America/Guatemala",
    "Eastern Time (US & Canada)" => "America/New_York",
    "Indiana (East)" => "America/Indiana/Indianapolis",
    "Bogota" => "America/Bogota",
    "Lima" => "America/Lima",
    "Quito" => "America/Lima",
    "Atlantic Time (Canada)" => "America/Halifax",
    "Caracas" => "America/Caracas",
    "La Paz" => "America/La_Paz",
    "Santiago" => "America/Santiago",
    "Asuncion" => "America/Asuncion",
    "Newfoundland" => "America/St_Johns",
    "Brasilia" => "America/Sao_Paulo",
    "Buenos Aires" => "America/Argentina/Buenos_Aires",
    "Montevideo" => "America/Montevideo",
    "Georgetown" => "America/Guyana",
    "Puerto Rico" => "America/Puerto_Rico",
    "Greenland" => "America/Nuuk",
    "Mid-Atlantic" => "Atlantic/South_Georgia",
    "Azores" => "Atlantic/Azores",
    "Cape Verde Is." => "Atlantic/Cape_Verde",
    "Dublin" => "Europe/Dublin",
    "Edinburgh" => "Europe/London",
    "Lisbon" => "Europe/Lisbon",
    "London" => "Europe/London",
    "Casablanca" => "Africa/Casablanca",
    "Monrovia" => "Africa/Monrovia",
    "UTC" => "Etc/UTC",
    "Belgrade" => "Europe/Belgrade",
    "Bratislava" => "Europe/Bratislava",
    "Budapest" => "Europe/Budapest",
    "Ljubljana" => "Europe/Ljubljana",
    "Prague" => "Europe/Prague",
    "Sarajevo" => "Europe/Sarajevo",
    "Skopje" => "Europe/Skopje",
    "Warsaw" => "Europe/Warsaw",
    "Zagreb" => "Europe/Zagreb",
    "Brussels" => "Europe/Brussels",
    "Copenhagen" => "Europe/Copenhagen",
    "Madrid" => "Europe/Madrid",
    "Paris" => "Europe/Paris",
    "Amsterdam" => "Europe/Amsterdam",
    "Berlin" => "Europe/Berlin",
    "Bern" => "Europe/Zurich",
    "Zurich" => "Europe/Zurich",
    "Rome" => "Europe/Rome",
    "Stockholm" => "Europe/Stockholm",
    "Vienna" => "Europe/Vienna",
    "West Central Africa" => "Africa/Algiers",
    "Bucharest" => "Europe/Bucharest",
    "Cairo" => "Africa/Cairo",
    "Helsinki" => "Europe/Helsinki",
    "Kyiv" => "Europe/Kyiv",
    "Riga" => "Europe/Riga",
    "Sofia" => "Europe/Sofia",
    "Tallinn" => "Europe/Tallinn",
    "Vilnius" => "Europe/Vilnius",
    "Athens" => "Europe/Athens",
    "Istanbul" => "Europe/Istanbul",
    "Minsk" => "Europe/Minsk",
    "Jerusalem" => "Asia/Jerusalem",
    "Harare" => "Africa/Harare",
    "Pretoria" => "Africa/Johannesburg",
    "Kaliningrad" => "Europe/Kaliningrad",
    "Moscow" => "Europe/Moscow",
    "St. Petersburg" => "Europe/Moscow",
    "Volgograd" => "Europe/Volgograd",
    "Samara" => "Europe/Samara",
    "Kuwait" => "Asia/Kuwait",
    "Riyadh" => "Asia/Riyadh",
    "Nairobi" => "Africa/Nairobi",
    "Baghdad" => "Asia/Baghdad",
    "Tehran" => "Asia/Tehran",
    "Abu Dhabi" => "Asia/Muscat",
    "Muscat" => "Asia/Muscat",
    "Baku" => "Asia/Baku",
    "Tbilisi" => "Asia/Tbilisi",
    "Yerevan" => "Asia/Yerevan",
    "Kabul" => "Asia/Kabul",
    "Ekaterinburg" => "Asia/Yekaterinburg",
    "Islamabad" => "Asia/Karachi",
    "Karachi" => "Asia/Karachi",
    "Tashkent" => "Asia/Tashkent",
    "Chennai" => "Asia/Kolkata",
    "Kolkata" => "Asia/Kolkata",
    "Mumbai" => "Asia/Kolkata",
    "New Delhi" => "Asia/Kolkata",
    "Kathmandu" => "Asia/Kathmandu",
    "Dhaka" => "Asia/Dhaka",
    "Sri Jayawardenepura" => "Asia/Colombo",
    "Almaty" => "Asia/Almaty",
    "Astana" => "Asia/Almaty",
    "Novosibirsk" => "Asia/Novosibirsk",
    "Rangoon" => "Asia/Yangon",
    "Bangkok" => "Asia/Bangkok",
    "Hanoi" => "Asia/Bangkok",
    "Jakarta" => "Asia/Jakarta",
    "Krasnoyarsk" => "Asia/Krasnoyarsk",
    "Beijing" => "Asia/Shanghai",
    "Chongqing" => "Asia/Chongqing",
    "Hong Kong" => "Asia/Hong_Kong",
    "Urumqi" => "Asia/Urumqi",
    "Kuala Lumpur" => "Asia/Kuala_Lumpur",
    "Singapore" => "Asia/Singapore",
    "Taipei" => "Asia/Taipei",
    "Perth" => "Australia/Perth",
    "Irkutsk" => "Asia/Irkutsk",
    "Ulaanbaatar" => "Asia/Ulaanbaatar",
    "Seoul" => "Asia/Seoul",
    "Osaka" => "Asia/Tokyo",
    "Sapporo" => "Asia/Tokyo",
    "Tokyo" => "Asia/Tokyo",
    "Yakutsk" => "Asia/Yakutsk",
    "Darwin" => "Australia/Darwin",
    "Adelaide" => "Australia/Adelaide",
    "Canberra" => "Australia/Canberra",
    "Melbourne" => "Australia/Melbourne",
    "Sydney" => "Australia/Sydney",
    "Brisbane" => "Australia/Brisbane",
    "Hobart" => "Australia/Hobart",
    "Vladivostok" => "Asia/Vladivostok",
    "Guam" => "Pacific/Guam",
    "Port Moresby" => "Pacific/Port_Moresby",
    "Magadan" => "Asia/Magadan",
    "Srednekolymsk" => "Asia/Srednekolymsk",
    "Solomon Is." => "Pacific/Guadalcanal",
    "New Caledonia" => "Pacific/Noumea",
    "Fiji" => "Pacific/Fiji",
    "Kamchatka" => "Asia/Kamchatka",
    "Marshall Is." => "Pacific/Majuro",
    "Auckland" => "Pacific/Auckland",
    "Wellington" => "Pacific/Auckland",
    "Nuku'alofa" => "Pacific/Tongatapu",
    "Tokelau Is." => "Pacific/Fakaofo",
    "Chatham Is." => "Pacific/Chatham",
    "Samoa" => "Pacific/Apia",
  }.freeze

  # Not ENV["TZ"] swapped for the block: that is process-wide, and a concurrent request would render in this one's zone.
  class TimeZoneData
    attr_reader :name

    def initialize(name, transitions, offsets, initial_offset, rule, dsts)
      @name = name
      @transitions = transitions
      @offsets = offsets
      @initial_offset = initial_offset
      @rule = rule
      @dsts = dsts
    end

    # Not `offset_at`: Rails' `TimeZone#utc_offset` is the period's BASE offset, the standard time without daylight saving.
    def base_offset_at(epoch)
      n = @transitions.length
      return @rule[0] if !@rule.empty? && (n == 0 || epoch >= @transitions[n - 1])
      return @initial_offset if n == 0 || epoch < @transitions[0]
      i = n - 1
      i = i - 1 while i > 0 && @transitions[i] > epoch
      i = i - 1 while i > 0 && @dsts[i]
      @dsts[i] ? @initial_offset : @offsets[i]
    end

    def offset_at(epoch)
      n = @transitions.length
      return rule_or(epoch, @initial_offset) if n == 0
      return @initial_offset if epoch < @transitions[0]
      return rule_or(epoch, @offsets[n - 1]) if epoch >= @transitions[n - 1]
      lo = 0
      hi = n - 1
      while lo < hi
        mid = (lo + hi + 1) / 2
        if @transitions[mid] <= epoch
          lo = mid
        else
          hi = mid - 1
        end
      end
      @offsets[lo]
    end

    # Past the last transition a slim TZif file leaves DST to its POSIX footer: [std, dst, m, w, d, secs, m, w, d, secs].
    def rule_or(epoch, fallback)
      return fallback if @rule.empty?
      return @rule[0] if @rule.length == 1
      std = @rule[0]
      dst = @rule[1]
      year = Time.at(epoch + std).utc.year
      start_at = ActiveSupport.rule_instant(year, @rule[2], @rule[3], @rule[4], @rule[5]) - std
      end_at = ActiveSupport.rule_instant(year, @rule[6], @rule[7], @rule[8], @rule[9]) - dst
      in_dst = start_at < end_at ? (epoch >= start_at && epoch < end_at) : !(epoch >= end_at && epoch < start_at)
      in_dst ? dst : std
    end
  end

  def self.rule_instant(year, month, week, wday, secs)
    first = civil_days(year, month, 1)
    day = 1 + (wday - (first + 4) % 7 + 7) % 7 + (week - 1) * 7
    last = days_in_month(year, month)
    day = day - 7 while day > last
    (first + day - 1) * 86_400 + secs
  end

  def self.be32(b, i)
    v = (b[i] << 24) | (b[i + 1] << 16) | (b[i + 2] << 8) | b[i + 3]
    v >= 2_147_483_648 ? v - 4_294_967_296 : v
  end

  def self.find_zone!(zone)
    raw = zone.to_s
    iana = ZONE_NAMES[raw] || raw
    dir = ENV["TZDIR"] || "/usr/share/zoneinfo"
    path = "#{dir}/#{iana}"
    unless iana.match?(/\A[A-Za-z0-9_+\-]+(\/[A-Za-z0-9_+\-]+)*\z/) && File.file?(path)
      raise ArgumentError, "Invalid Timezone: #{raw}"
    end
    b = File.binread(path).bytes
    raise ArgumentError, "Invalid Timezone: #{raw}" unless b.length > 44 && b[0] == 84 && b[1] == 90 && b[2] == 105 && b[3] == 102
    base = 0
    width = 4
    if b[4] >= 50
      base = 44 + be32(b, 32) * 5 + be32(b, 36) * 6 + be32(b, 40) + be32(b, 28) * 8 + be32(b, 24) + be32(b, 20)
      width = 8
    end
    leapcnt = be32(b, base + 28)
    timecnt = be32(b, base + 32)
    typecnt = be32(b, base + 36)
    charcnt = be32(b, base + 40)
    at = base + 44
    transitions = []
    i = 0
    while i < timecnt
      transitions << (width == 8 ? be32(b, at + i * 8) * 4_294_967_296 + (be32(b, at + i * 8 + 4) & 0xffffffff) : be32(b, at + i * 4))
      i = i + 1
    end
    idx_at = at + timecnt * width
    types_at = idx_at + timecnt
    offsets = []
    dsts = []
    i = 0
    while i < timecnt
      offsets << be32(b, types_at + b[idx_at + i] * 6)
      dsts << (b[types_at + b[idx_at + i] * 6 + 4] == 1)
      i = i + 1
    end
    rule = []
    if width == 8
      footer_at = types_at + typecnt * 6 + charcnt + leapcnt * 12 + be32(b, base + 24) + be32(b, base + 20)
      rule = parse_tz_rule(File.binread(path)[footer_at..].to_s.strip)
    end
    TimeZoneData.new(iana, transitions, offsets, be32(b, types_at), rule, dsts)
  end

  # Only the `Mm.w.d` rule form: a `Jn` / `n` footer yields no rule, and the last transition's offset stands.
  def self.parse_tz_rule(spec)
    m = /\A(?:<[^>]+>|[A-Za-z]+)([+-]?\d+(?::\d+){0,2})(?:(?:<[^>]+>|[A-Za-z]+)([+-]?\d+(?::\d+){0,2})?,M(\d+)\.(\d)\.(\d)(?:\/([+-]?\d+(?::\d+){0,2}))?,M(\d+)\.(\d)\.(\d)(?:\/([+-]?\d+(?::\d+){0,2}))?)?\z/.match(spec)
    return [] if m.nil?
    std = -posix_seconds(m[1])
    return [std] if m[3].nil?
    dst = m[2] ? -posix_seconds(m[2]) : std + 3600
    start_secs = m[6] ? posix_seconds(m[6]) : 7200
    end_secs = m[10] ? posix_seconds(m[10]) : 7200
    [std, dst, m[3].to_i, m[4].to_i, m[5].to_i, start_secs, m[7].to_i, m[8].to_i, m[9].to_i, end_secs]
  end

  def self.posix_seconds(text)
    sign = text.start_with?("-") ? -1 : 1
    parts = text.delete("+-").split(":")
    h = parts[0].to_i
    mi = parts.length > 1 ? parts[1].to_i : 0
    s = parts.length > 2 ? parts[2].to_i : 0
    sign * (h * 3600 + mi * 60 + s)
  end

  # The app's `config.time_zone` (Rails' default `Time.zone`), set once at
  # boot by `set_default_zone`; empty until then, which presents in the
  # host's zone. Rails keeps the process zone alone: `Time.now` is the
  # host's local time and only `Time.current` and Active Record values are
  # in the configured zone. The boot used to pin `ENV["TZ"]` to the
  # configured zone instead, which made `Time.now` the configured zone
  # too (an app that names a file after `Time.now.strftime` on a Warsaw
  # host gets the Warsaw time from Rails).
  DEFAULT_ZONE = []

  # UTC needs no tzdata: a zone with no transitions and offset 0, so a
  # container without /usr/share/zoneinfo still boots on Rails' default.
  def self.set_default_zone(name)
    iana = ZONE_NAMES[name.to_s] || name.to_s
    DEFAULT_ZONE.clear
    if iana == "UTC" || iana == "Etc/UTC" || iana == "Etc/UCT" || iana == "GMT" || iana == "Etc/GMT"
      DEFAULT_ZONE << TimeZoneData.new(name.to_s, [], [], 0, [], [])
    else
      DEFAULT_ZONE << find_zone!(name)
    end
    nil
  end

  def self.current_zone
    zone = Thread.current[:rh_time_zone]
    zone.nil? ? DEFAULT_ZONE[0] : zone
  end

  def self.use_zone(zone)
    previous = Thread.current[:rh_time_zone]
    Thread.current[:rh_time_zone] = zone.nil? ? nil : find_zone!(zone)
    begin
      yield
    ensure
      Thread.current[:rh_time_zone] = previous
    end
  end

  def self.present(t)
    zone = current_zone
    zone.nil? ? t.getlocal : present_in_zone(t, zone)
  end

  def self.present_in_zone(t, zone)
    return t.getutc if ActiveSupport::JSON.utc_zone_name?(zone.name)
    t.getlocal(zone.offset_at(t.to_i))
  end

  def self.present_db(t)
    return nil if t.nil?
    present(t)
  end

  def self.in_time_zone(t, zone)
    return present(t) if zone.nil?
    present_in_zone(t, find_zone!(zone))
  end

  def self.zone_name(zone)
    zone.nil? ? nil : zone.name
  end

  # Numeric#in_time_zone: epoch seconds before the zone remapping above.
  def self.time_at_epoch(n)
    Time.at(n)
  end

  # Not `Time.local`: under `use_zone` the civil value belongs to that zone, resolved twice to settle an offset change.
  def self.local_time(y, mo, d, h, mi, s, nsec)
    zone = current_zone
    return Time.at(Time.local(y, mo, d, h, mi, s).to_i, nsec, :nsec) if zone.nil?
    guess = Time.utc(y, mo, d, h, mi, s).to_i
    epoch = guess - zone.offset_at(guess - zone.offset_at(guess))
    present_in_zone(Time.at(epoch, nsec, :nsec), zone)
  end

  # Not `number.to_s` with commas everywhere: only an all-digit integer part takes them, so `1.0e+20` and `Infinity` pass through.
  def self.number_delimited(number)
    text = number.to_s
    dot_at = text.index(".")
    int = dot_at.nil? ? text : text[0, dot_at].to_s
    rest = dot_at.nil? ? "" : text[dot_at, text.length - dot_at].to_s
    sign = int.start_with?("-") ? "-" : ""
    digits = sign == "" ? int : int[1, int.length - 1].to_s
    return text unless digits.match?(/\A\d+\z/)
    out = +""
    i = 0
    n = digits.length
    while i < n
      out << "," if i > 0 && (n - i) % 3 == 0
      out << digits[i]
      i = i + 1
    end
    sign + out + rest
  end

  # Not `f.to_s` as it is: `Float#to_d` keeps the shortest digits but cuts them at 16 without rounding (bigdecimal's BIGDECIMAL_DOUBLE_FIGURES).
  def self.float_decimal_text(f)
    text = f.to_s
    exp_at = text.index("e")
    mantissa = exp_at.nil? ? text : text[0, exp_at].to_s
    exp = exp_at.nil? ? 0 : text[exp_at + 1, text.length].to_s.to_i
    negative = mantissa.start_with?("-")
    mantissa = mantissa[1, mantissa.length].to_s if negative
    dot = mantissa.index(".")
    int_part = dot.nil? ? mantissa : mantissa[0, dot].to_s
    frac_part = dot.nil? ? "" : mantissa[dot + 1, mantissa.length].to_s
    digits = int_part + frac_part
    lead = 0
    while lead < digits.length && digits[lead] == "0"
      lead = lead + 1
    end
    significant = digits[lead, digits.length].to_s
    return text if significant.length <= 16
    point = int_part.length - lead + exp
    (negative ? "-" : "") + "0." + significant[0, 16].to_s + "e" + point.to_s
  end

  def self.rails_zone_names
    ZONE_NAMES.keys
  end

  # `Time#formatted_offset`: Rails spells the offset "+09:00", "+00:00" for UTC.
  def self.formatted_offset(t)
    ActiveSupport.seconds_to_utc_offset(t.utc_offset)
  end

  def self.seconds_to_utc_offset(seconds)
    sign = seconds < 0 ? "-" : "+"
    abs = seconds.abs
    hours = abs / 3600
    minutes = (abs % 3600) / 60
    sign + (hours < 10 ? "0" : "") + hours.to_s + ":" + (minutes < 10 ? "0" : "") + minutes.to_s
  end

  class TimeZone
    attr_reader :name

    def initialize(name, data)
      @name = name
      @data = data
    end

    # Not `[]`: a class-side index reads as an operator on the strict targets; `lower::time_calendar` spells `TimeZone[x]` this way.
    def self.lookup(name)
      return nil if name.nil? || name.to_s.empty?
      begin
        TimeZone.new(name.to_s, ActiveSupport.find_zone!(name.to_s))
      rescue ArgumentError
        nil
      end
    end

    # Rails' `zones_map` (`activesupport/lib/active_support/values/time_zone.rb`)
    # builds this by calling `self[name]` per mapped name and keeping only
    # the ones that resolve; `[]` itself rescues `TZInfo::InvalidTimezoneIdentifier`
    # and returns nil for the rest. `lookup` is our spelling of that `[]`.
    # A host missing a legacy zone file (no `tzdata-legacy`) then just has
    # fewer zones, not a raise.
    def self.all
      zones = []
      ActiveSupport.rails_zone_names.each do |zone_name|
        zone = lookup(zone_name)
        zones << zone unless zone.nil?
      end
      # Not `sort { |a, b| … }`: a block's parameters do not type on every target, and 152 zones sort cheaply in place.
      i = 1
      while i < zones.length
        current = zones[i]
        j = i - 1
        while j >= 0 && zones[j].compare(current) > 0
          zones[j + 1] = zones[j]
          j = j - 1
        end
        zones[j + 1] = current
        i = i + 1
      end
      zones
    end

    def compare(other)
      by_offset = utc_offset <=> other.utc_offset
      by_offset == 0 ? (name <=> other.name) : by_offset
    end

    def utc_offset
      @data.base_offset_at(Time.now.to_i)
    end

    def formatted_offset
      ActiveSupport.seconds_to_utc_offset(utc_offset)
    end

    def to_s
      "(GMT" + formatted_offset + ") " + @name
    end

    def now
      t = Time.now
      ActiveSupport.present_in_zone(t, @data)
    end
  end
end
