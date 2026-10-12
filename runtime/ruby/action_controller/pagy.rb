# The `pagy` gem (43.6.2 -- the version EngineeredAt's Gemfile.lock pins):
# `Pagy::Method#pagy` and the offset paginator it returns.
#
# The gem hands a controller `pagy(scope, limit: n)`, which answers
# `[pagy, records]`; the view reads the `pagy` object (`@pagy.last`,
# `@pagy.page`, `@pagy.count`) and renders `@pagy.series_nav` /
# `@pagy.info_tag`. This file is that surface for the OFFSET paginator,
# nothing more, and its markup is the gem's, byte for byte (the oracle is
# work/_scratch/pagy-oracle/oracle.rb: the same calls on the real gem).
#
# Ruby-family only, like pagination.rb beside it (geared_pagination): the
# file is a `Base` reopen that is not in any strict-target table, and
# project.rs drops it unless the app's Gemfile.lock carries pagy. The
# gem's `Pagy::Method` is a module an app includes; here `pagy` lives on
# `Base` itself, because the include that matters sits inside a concern's
# `included do` block (EngineeredAt's `Pagination`), which no target
# replays. `Pagy::Method` is therefore an empty marker so that
# `include Pagy::Method` resolves.
#
# WHAT IS MODELED
#
# - `pagy(scope)` and `pagy(scope, limit: n)`; `page` from the query
#   string's `page` key, floored at 1 exactly as `String#to_i` reads it
#   (`2abc` is 2, `abc` and `-3` are 1); a page past the end is an EMPTY
#   page with `previous` = last, as the gem answers it (the app's own
#   404 is `Pagination::BoundsCheck`, prepended on top of this).
# - the readers `page limit count offset last pages from to previous
#   next`, `series_nav` (7 slots, the default `pagy` style, English) and
#   `info_tag` with no arguments.
# - links keep the request's path and every other query pair, in order,
#   canonically re-encoded (`URI.encode_www_form_component`), with the
#   `page` pair replaced in place or appended and the `limit` pair
#   dropped, as `compose_page_url` does with `max_limit` unset.
#
# WHAT IS NOT, and how each is refused
#
# - Options other than a literal `limit:`, a paginator other than the
#   default offset one, and a non-relation collection: refused at
#   analysis, with a diagnostic naming the option (analyze/body: pagy).
# - Methods and arguments the object does not have here (`page_url`,
#   `series_nav(id: ...)`, `info_tag(item_name: ...)`, `in`, `options`,
#   `series_nav_js`, ...): no known method on Pagy, at analysis.
# - Locale: the gem's `Pagy::I18n.locale` is "en" unless the app sets it;
#   no other dictionary is carried.
# - A query string the gem would parse into nested params (a key with
#   `[` or `]`): raises NotImplementedError rather than rebuilding the
#   link differently. A malformed `%` escape raises ArgumentError (Rack
#   answers 400 for it).

# `Pagy::Method` for `include Pagy::Method`: see the header.
class Pagy
  module Method
  end

  # The gem's argument error (`expected :limit >= 1; got 0`).
  class OptionError < ArgumentError
  end

  SERIES_SLOTS = 7
  HEX = "0123456789ABCDEF"

  # The fields are computed through LOCALS and stored once, and the
  # derived methods read through the declared READERS, for the reason
  # pagination.rb gives: an ivar's body type is nullable, a reader's is
  # not.
  def initialize(count, page, limit, path, query)
    raise OptionError, "expected :limit >= 1; got #{limit}" if limit < 1
    raise OptionError, "expected :count >= 0; got #{count}" if count < 0
    last_page = (count + limit - 1) / limit
    last_page = 1 if last_page < 1
    first_row = limit * (page - 1)
    from_row = 0
    to_row = 0
    in_page = 0
    before = nil
    after = nil
    if page <= last_page
      from_row = first_row + 1
      from_row = count if count < from_row
      to_row = first_row + limit
      to_row = count if count < to_row
      in_page = to_row - from_row + 1
      in_page = count if count < in_page
      before = page - 1 unless page == 1
      after = page + 1 unless page == last_page
    else
      before = last_page
    end
    @page = page
    @limit = limit
    @count = count
    @offset = first_row
    @last = last_page
    @from = from_row
    @to = to_row
    @in_page = in_page
    @previous = before
    @next = after
    split = Pagy.split_query(query)
    head = split[0].join("&")
    tail = split[1].join("&")
    left = path + "?" + head + (head.empty? ? "" : "&") + "page="
    right = tail.empty? ? "" : "&" + tail
    @url_left = Pagy.escape_html(left)
    @url_right = Pagy.escape_html(right)
  end

  def page
    @page
  end

  def limit
    @limit
  end

  def count
    @count
  end

  def offset
    @offset
  end

  def last
    @last
  end

  def pages
    last
  end

  def from
    @from
  end

  def to
    @to
  end

  def previous
    @previous
  end

  def next
    @next
  end

  def in_page
    @in_page
  end

  # The gem's `info_tag` with no arguments (English dictionary).
  def info_tag
    total = count
    name = total == 1 ? "item" : "items"
    text = if total == 0
      "No items found"
    elsif in_page == total
      "Displaying #{total} #{name}"
    else
      "Displaying #{name} #{from}-#{to} of #{total} in total"
    end
    %(<span class="pagy info">#{text}</span>)
  end

  # The gem's `series_nav` with no arguments: the `pagy` style, 7 slots.
  def series_nav
    html = +""
    html << previous_tag
    series_items.each do |item|
      if item == -1
        html << %(<a role="separator" aria-disabled="true">&hellip;</a>)
      elsif item == page
        html << %(<a role="link" aria-disabled="true" aria-current="page">#{item}</a>)
      else
        html << anchor(item, item.to_s, "")
      end
    end
    html << next_tag
    label = last == 1 ? "Page" : "Pages"
    %(<nav class="pagy series-nav" aria-label="#{label}">#{html}</nav>)
  end

  # Page numbers with -1 for a gap: the gem's `series` for 7 slots,
  # `compact` unset, e.g. `[1, -1, 7, 8, 9, 10, 11, -1, 36]` (the current
  # page is among the numbers; series_nav marks it).
  def series_items
    last_page = last
    current = page
    items = []
    if SERIES_SLOTS >= last_page
      n = 1
      while n <= last_page
        items << n
        n += 1
      end
    else
      half = (SERIES_SLOTS - 1) / 2
      start = current - half
      if current <= half
        start = 1
      elsif current > last_page - SERIES_SLOTS + half
        start = last_page - SERIES_SLOTS + 1
      end
      n = 0
      while n < SERIES_SLOTS
        items << start + n
        n += 1
      end
      items[0] = 1
      items[1] = -1 unless items[1] == 2
      items[SERIES_SLOTS - 2] = -1 unless items[SERIES_SLOTS - 2] == last_page - 1
      items[SERIES_SLOTS - 1] = last_page
    end
    items
  end

  def previous_tag
    target = previous
    return %(<a role="link" aria-disabled="true" aria-label="Previous">&lt;</a>) if target.nil?
    anchor(target, "&lt;", "Previous")
  end

  def next_tag
    target = self.next
    return %(<a role="link" aria-disabled="true" aria-label="Next">&gt;</a>) if target.nil?
    anchor(target, "&gt;", "Next")
  end

  # One `<a>`: `rel` is prev/next when the target is the previous/next
  # page, then the aria-label when there is one -- the gem's attribute
  # order.
  def anchor(target, text, aria_label)
    rel = ""
    rel = %( rel="prev") if target == previous
    rel = %( rel="next") if target == self.next
    aria = aria_label.empty? ? "" : %( aria-label="#{aria_label}")
    %(<a href="#{@url_left}#{target}#{@url_right}"#{rel}#{aria}>#{text}</a>)
  end

  # `CGI.escapeHTML`.
  def self.escape_html(text)
    text.gsub("&", "&amp;").gsub("<", "&lt;").gsub(">", "&gt;").gsub("\"", "&quot;").gsub("'", "&#39;")
  end

  # One byte, as `URI.encode_www_form_component` writes it: the
  # unreserved set `* - . _ 0-9 A-Z a-z` as is, a space as `+`, the rest
  # as `%XX`.
  def self.encode_byte(b)
    if (b >= 48 && b <= 57) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122) || b == 42 || b == 45 || b == 46 || b == 95
      b.chr
    elsif b == 32
      "+"
    else
      "%" + HEX[b / 16].to_s + HEX[b % 16].to_s
    end
  end

  def self.hex_digit(b)
    return b - 48 if b >= 48 && b <= 57
    return b - 55 if b >= 65 && b <= 70
    return b - 87 if b >= 97 && b <= 102
    -1
  end

  # The byte one query-string byte, or the `%XX` run starting at `i`,
  # stands for (`+` is a space); -1 for a malformed escape.
  def self.decoded_byte(raw, i)
    b = raw.getbyte(i).to_i
    return 32 if b == 43
    return b unless b == 37
    return -1 if i + 2 >= raw.bytesize
    hi = hex_digit(raw.getbyte(i + 1).to_i)
    lo = hex_digit(raw.getbyte(i + 2).to_i)
    return -1 if hi < 0 || lo < 0
    hi * 16 + lo
  end

  # Decode then re-encode (`encode_www_form_component(decode_www_form_
  # component(raw))`), byte by byte.
  def self.canonical(raw)
    out = +""
    i = 0
    n = raw.bytesize
    while i < n
      d = decoded_byte(raw, i)
      raise ArgumentError, "invalid %-encoding (#{raw})" if d < 0
      out << encode_byte(d)
      i += raw.getbyte(i).to_i == 37 ? 3 : 1
    end
    out
  end

  # The decoded text of a value, for `String#to_i` only: ASCII bytes as
  # themselves, anything else as `?` (a non-digit either way).
  def self.decoded_text(raw)
    out = +""
    i = 0
    n = raw.bytesize
    while i < n
      d = decoded_byte(raw, i)
      raise ArgumentError, "invalid %-encoding (#{raw})" if d < 0
      out << (d < 128 ? d.chr : "?")
      i += raw.getbyte(i).to_i == 37 ? 3 : 1
    end
    out
  end

  # `request.GET` the way the gem reads it, for a flat query: the pairs
  # in first-seen order, a repeated key keeping its first position and
  # its LAST value, a bare `k` staying `k` and `k=` staying `k=`, empty
  # segments and empty keys dropped. Returns
  # `[pairs before page, pairs after page]` with the `page` pair
  # removed (the link writes its own) and any `limit` pair dropped.
  def self.split_query(query)
    keys = []
    pairs = []
    query.split("&").each do |segment|
      next if segment.empty?
      eq = segment.index("=")
      raw_key = eq.nil? ? segment : segment[0, eq].to_s
      key = canonical(raw_key)
      next if key.empty?
      if key.include?("%5B") || key.include?("%5D")
        raise NotImplementedError, "pagy: a query key with [ or ] is a nested parameter; links for it are not modeled (#{raw_key})"
      end
      pair = eq.nil? ? key : key + "=" + canonical(segment[eq + 1, segment.length].to_s)
      at = keys.index(key)
      if at.nil?
        keys << key
        pairs << pair
      else
        pairs[at] = pair
      end
    end
    head = []
    tail = []
    seen_page = false
    keys.each_with_index do |key, i|
      next if key == "limit"
      if key == "page"
        seen_page = true
        next
      end
      if seen_page
        tail << pairs[i].to_s
      else
        head << pairs[i].to_s
      end
    end
    [head, tail]
  end

  # `Pagy::Request#resolve_page`: `params["page"].to_s.to_i`, at least 1.
  def self.page_param(query)
    value = nil
    query.split("&").each do |segment|
      next if segment.empty?
      eq = segment.index("=")
      raw_key = eq.nil? ? segment : segment[0, eq].to_s
      next unless canonical(raw_key) == "page"
      value = eq.nil? ? "" : segment[eq + 1, segment.length].to_s
    end
    return 1 if value.nil?
    n = decoded_text(value).to_i
    n < 1 ? 1 : n
  end
end

module ActionController
  class Base
    # `Pagy::Method#pagy` for the offset paginator: `[pagy, records]`.
    # `options` is a Hash, not keywords, so that a prepended wrapper's
    # `super` (which forwards the hash it was handed positionally)
    # reaches it the way a direct call does. Only `:limit` is read; the
    # analysis refuses every other key before this runs.
    def pagy(collection, options = {})
      limit = options.fetch(:limit, 20)
      # `request.path` (SCRIPT_NAME + PATH_INFO) and the raw query string,
      # as the gem's Pagy::Request reads them. A dispatcher that built no
      # request object still seeds `request_path` and `query_string`.
      path = request_path.split("?", 2)[0].to_s
      query = query_string
      current = request
      unless current.nil?
        path = current.script_name + current.path
        query = current.query_string
      end
      page = Pagy.page_param(query)
      total = collection.count
      paginator = Pagy.new(total, page, limit, path, query)
      [paginator, collection.offset(paginator.offset).limit(paginator.limit)]
    end
  end
end
