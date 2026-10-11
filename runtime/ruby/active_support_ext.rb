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
  # numbers, Strings, Symbols, and Arrays/Hashes of them, keys as their
  # `to_s`, a non-finite Float as null, as Rails answers. Anything else
  # (a Time, a record) raises rather than encode some other text.
  #
  # `decode` is `JSON.parse`, which is all Rails' is while
  # `ActiveSupport.parse_json_times` keeps its default (off).
  module JSON
    def self.encode(value)
      JsonBuilder.escape_html_entities(::JSON.generate(ActiveSupport::JSON.jsonable(value)))
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
      when nil, true, false, Integer, String then value
      else raise ArgumentError, "ActiveSupport::JSON.encode: #{value.class} values are not supported yet"
      end
    end
  end

  # ActiveSupport's `deep_dup` (core_ext/object/deep_dup.rb), for a typed
  # Hash or Array receiver (`lower::symbolize_keys` routes the call here).
  # A Hash copies each value and, for keys that are neither String nor
  # Symbol, the key too — re-inserted after the String/Symbol ones, as
  # Rails' delete-and-store does. An Array copies each element. Any other
  # value is its `dup` (an Integer's, nil's or a Symbol's is itself).
  def self.deep_dup(value)
    case value
    when Array then value.map { |item| ActiveSupport.deep_dup(item) }
    when Hash
      out = {}
      value.each { |key, item| out[key] = ActiveSupport.deep_dup(item) if key.is_a?(String) || key.is_a?(Symbol) }
      value.each { |key, item| out[ActiveSupport.deep_dup(key)] = ActiveSupport.deep_dup(item) unless key.is_a?(String) || key.is_a?(Symbol) }
      out
    else value.dup
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
    list.include?(value.to_s) ? value : nil
  end

  # Rails' `ActiveModel::Errors#[]` — the messages for ONE attribute,
  # without the humanized attribute prefix (`[ "is not public" ]`, not
  # `[ "Url is not public" ]`). Reached from `src/lower/errors_index.rs`,
  # which passes the prefix the `errors.add` / `validates` lowerings
  # baked in ("Url ", trailing space included so `url` cannot match
  # `url_host`'s "Url host …" on the space boundary alone).
  #
  # Takes the accumulator as an ARGUMENT for the same reason `blank?`
  # above does: the shared accumulator is a plain `Array[String]` and
  # Array has no `[]`-by-Symbol to reopen, so the projection has to be a
  # function over the array rather than a method on it.
  #
  # `m[prefix.length, m.length - prefix.length]` rather than a range or
  # `delete_prefix`: two-integer `String#[]` is the slice spelling every
  # target lowers, and `sub`/`delete_prefix` are the shapes that have
  # bitten this runtime before (a two-string `gsub` has no C# lowering).
  def self.errors_for(errors, prefix)
    out = []
    errors.each do |m|
      out << m[prefix.length, m.length - prefix.length] if m.start_with?(prefix)
    end
    out
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
  # including Unicode whitespace.
  #
  # campfire's `content_filters_test` writes `<<~HTML.squish` to put a
  # multi-line fixture body on one line before handing it to a filter.
  def self.squish(text)
    out = +""
    pending_space = false
    i = 0
    while i < text.length
      c = text[i]
      if c.match?(/[[:space:]]/)
        pending_space = !out.empty?
      else
        out = out + " " if pending_space
        pending_space = false
        out = out + c
      end
      i = i + 1
    end
    out
  end

  # Rails' destructive form always returns the same receiver, including
  # when its contents were already squished.
  def self.squish!(text)
    text.gsub!(/[[:space:]]+/, " ")
    text.strip!
    text
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
  def self.cast_boolean(value)
    return nil if value.nil?
    text = value.to_s
    return nil if text == ""
    !%w[0 f F false FALSE off OFF].include?(text)
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
    "Kyiv" => "Europe/Kiev",
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
    "Rangoon" => "Asia/Rangoon",
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

  def self.current_zone
    Thread.current[:rh_time_zone]
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
    zone.nil? ? t.getlocal : t.getlocal(zone.offset_at(t.to_i))
  end

  def self.present_db(t)
    return nil if t.nil?
    present(t)
  end

  def self.in_time_zone(t, zone)
    return present(t) if zone.nil?
    t.getlocal(find_zone!(zone).offset_at(t.to_i))
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
    Time.at(epoch, nsec, :nsec).getlocal(zone.offset_at(epoch))
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

    def self.all
      zones = []
      ActiveSupport.rails_zone_names.each do |zone_name|
        zones << TimeZone.new(zone_name, ActiveSupport.find_zone!(zone_name))
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
      t.getlocal(@data.offset_at(t.to_i))
    end
  end
end
