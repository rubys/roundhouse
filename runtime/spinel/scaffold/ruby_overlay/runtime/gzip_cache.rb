# frozen_string_literal: true

# Cache of gzip(identity body) for the CRuby overlay.
#
# Rack::Deflater compresses every response. A campfire room page is the
# same ~420 KB HTML for every wrk GET that shares a session, so that is
# the same deflate over and over.
#
# Hit path, measured on 420 KB identity HTML:
#   * `join_body` of Rack `[body]` must not `join` (that copied 420 KB).
#   * Last-identity compare (`bytesize` then `==`) is ~7 µs; MRI string
#     hash of a fresh 420 KB body is ~294 µs. wrk hammers one URL, so
#     the last-hit wins.
#   * The Hash fallback keys by the body's String#hash, its size and its
#     CRC-32 — not the identity bytes (holding 64 × 420 KB strings as Hash
#     keys was the RSS cost). A wrong-body hit needs a 64-bit hash, the
#     size and a CRC-32 to collide at once. It used to be SHA-256, which
#     at 3.5 ms per 460 KB (2 GHz) was a quarter of a request that missed
#     — and a page with a per-request token always misses. Both String#hash
#     and Zlib.crc32 are C and run once each per miss.
#
# Gzip itself runs outside the lock. HTML only, same skips as tep.
#
# SPLICING. A page that varies per request (a fresh masked CSRF token in
# the layout) misses both caches above, and then paid for the whole page:
# on campfire's ~460 KB room page, SHA-256 (3.5 ms) plus level-6 gzip
# (6.2 ms) of 13.3 ms per request at 2 GHz. Most of that page is ONE
# cached fragment (the message collection, ~400 KB) that is the same frozen
# String on every request until a message changes. So, after the once-
# campfire Ruby port's page_parts.rb (itself after the Rust port's
# deflater/splice.rs):
#   * the fragment cache notes each large fragment it hands out during the
#     request (Recorder, prepended onto the overlay's Rails::MemoryStore);
#   * each fragment is deflated once per (predecessor chain, glue), raw,
#     SYNC_FLUSHed to a byte boundary, with the run's preceding bytes (up
#     to deflate's 32 KB window) as its preset dictionary — consecutive
#     messages share HTML, and without that dictionary a room of parts is
#     ~4× larger than a whole-page gzip. Kept with its CRC-32 in a
#     WeakKeyMap (keys are not held alive: a replaced fragment's piece
#     goes with it). A piece is only reused when the same predecessors
#     (by identity) and the same glue precede it, so a wrong-body hit is
#     refused even when CSRF in the layout varies;
#   * short glue (≤ MAX_GLUE) between consecutive fragments travels with
#     the second when it holds no recorded request token; token-bearing
#     or longer gaps go through splice_text and start a new run;
#   * per request only the text between runs is deflated, at BEST_SPEED,
#     with the real preceding 32 KB as its dictionary;
#   * the pieces are framed as one gzip member: header, pieces, a final
#     empty block, CRC-32 (crc32_combine) and length.
# Fragments are FOUND in the final body (byteindex, in recording order)
# rather than tracked by offset: a view renders into its own buffer and
# the layout appends that buffer, so an offset taken while rendering is
# not an offset in the body. A fragment nested inside a later one (a
# collection miss writes its members, then the collection) is found in
# order and the container skipped. No fragment found: the paths above.
#
# The text between fragments is mostly the layout, the same bytes on every
# request except for the masked CSRF tokens in it (a fresh pad each
# render): 20-30 KB of text deflated per request for 86-byte differences.
# So each token the request minted is noted too (TokenRecorder), the text
# is cut at the tokens, and each constant run between them is deflated once
# and cached by its bytes, like a fragment. A token goes out as a stored
# block: random base64 does not compress. Correctness never depends on
# finding every token: a run is looked up by its bytes, so a run holding
# an unnoticed token just misses and is deflated, as before.
require "weakref"
require "zlib"

module GzipCache
  MAX_ENTRIES = 64

  @store = {}
  @mutex = Mutex.new
  @last_raw = nil
  @last_gz = nil

  def self.wrap(app)
    lambda { |env| call(app, env) }
  end

  def self.call(app, env)
    prev = Thread.current[:rh_gzip_fragments]
    prev_tokens = Thread.current[:rh_gzip_tokens]
    Thread.current[:rh_gzip_fragments] = []
    Thread.current[:rh_gzip_tokens] = []
    begin
      status, headers, body = app.call(env)
      fragments = Thread.current[:rh_gzip_fragments]
      tokens = Thread.current[:rh_gzip_tokens]
    ensure
      Thread.current[:rh_gzip_fragments] = prev
      Thread.current[:rh_gzip_tokens] = prev_tokens
    end
    maybe_gzip(env, status, headers, body, fragments, tokens)
  end

  def self.maybe_gzip(env, status, headers, body, fragments = nil, tokens = nil)
    return [status, headers, body] if status < 200 || status == 204 || status == 304
    return [status, headers, body] if env["REQUEST_METHOD"] == "HEAD"
    accept = env["HTTP_ACCEPT_ENCODING"].to_s
    return [status, headers, body] unless accepts_gzip?(accept)
    return [status, headers, body] if header(headers, "content-encoding")
    raw = join_body(body)
    return [status, headers, [raw]] if raw.bytesize < 64
    return [status, headers, [raw]] if binary?(header(headers, "content-type"))
    gz = compress(raw, fragments, tokens)
    headers = headers.dup
    headers["content-encoding"] = "gzip"
    vary = header(headers, "vary")
    if vary.nil? || vary.empty?
      headers["vary"] = "Accept-Encoding"
    elsif !vary.downcase.include?("accept-encoding")
      headers["vary"] = "#{vary}, Accept-Encoding"
    end
    headers["content-length"] = gz.bytesize.to_s
    [status, headers, [gz]]
  end

  def self.compress(raw, fragments = nil, tokens = nil)
    @mutex.synchronize do
      lr = @last_raw
      if !lr.nil? && lr.bytesize == raw.bytesize && lr == raw
        return @last_gz
      end
    end
    has_frags = !fragments.nil? && !fragments.empty?
    has_tokens = !tokens.nil? && !tokens.empty?
    if has_frags || has_tokens
      spliced = splice(raw, has_frags ? fragments : [], has_tokens ? tokens : nil)
      return spliced unless spliced.nil?
    end
    dig = [raw.hash, raw.bytesize, Zlib.crc32(raw)]
    hit = nil
    @mutex.synchronize do
      hit = @store[dig]
    end
    return hit unless hit.nil?
    gz = Zlib.gzip(raw)
    @mutex.synchronize do
      if @store.size >= MAX_ENTRIES
        @store.clear
      end
      @store[dig] = gz
      # Snapshot: the Rack body string can be reused and mutated
      # between requests. Sharing it would make last-hit `==` match
      # the mutated bytes while `@last_gz` is still the old gzip.
      @last_raw = raw.dup
      @last_gz = gz
    end
    gz
  end

  # ── Splicing ──

  # Fragments smaller than this are left in the per-request text: a piece
  # costs a WeakKeyMap lookup (one hash of the fragment) and a flush.
  SPLICE_MIN = 1024
  # Deflate's window, and so the most preset dictionary that can matter.
  WINDOW = 32 * 1024
  # At most this much text between two fragments travels with the second
  # (ports' MAX_GLUE). More is layout: a new run, no predecessor dict.
  MAX_GLUE = 256
  # Pieces kept per fragment, one per predecessor chain seen (room page,
  # older page, search results) — same bound as the ports.
  PIECES_PER_FRAGMENT = 4
  # Text pieces shorter than this go out as stored (uncompressed) deflate
  # blocks. Between consecutive fragments (search results, a message list)
  # the text is a few bytes of glue, and a Deflate per piece spent its time
  # in set_dictionary on 32 KB it would barely use: 26% of the search page.
  STORE_MAX = 1024
  # gzip member header: deflate, no flags, mtime 0, no extra flags, Unix.
  GZIP_HEADER = "\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03".b.freeze
  # A final, empty, fixed-Huffman block: ends the deflate stream after
  # pieces that each end on a non-final SYNC_FLUSH block.
  FINAL_BLOCK = "\x03\x00".b.freeze
  EMPTY = "".b.freeze
  SPLICE_OK = Zlib.respond_to?(:crc32_combine) && defined?(ObjectSpace::WeakKeyMap) ? true : false

  # JRuby's zlib can splice raw deflate pieces but rejects a preset
  # dictionary on a raw stream. A dictionary improves compression only;
  # independent pieces still inflate correctly without it. Probe once,
  # using a disposable stream rather than a failed request's compressor.
  RAW_DICTIONARY_OK = SPLICE_OK && begin
    probe = Zlib::Deflate.new(Zlib::BEST_SPEED, -Zlib::MAX_WBITS)
    begin
      probe.set_dictionary("roundhouse")
      true
    rescue Zlib::StreamError
      false
    ensure
      probe.close
    end
  end

  # frag -> list of [chain, glue, deflated, crc]. Chain names the
  # predecessors (and their glue) the piece was deflated against.
  @pieces = SPLICE_OK ? ObjectSpace::WeakKeyMap.new : nil
  @pieces_mutex = Mutex.new
  # The last fragment looked up and its entry. The cache hands a view the
  # same frozen String until a record changes, so most lookups are this
  # one: an identity check instead of WeakKeyMap hashing ~400 KB (4.6% of
  # the room page). Entry shape matches a @pieces variant.
  @last_piece = nil
  # The last splice: [fragments, texts, gzip]. A page that comes back with
  # the same text around the same fragments (one with no per-request token)
  # reuses it without deflating anything.
  @last_splice = nil
  # How much of a fragment's head is searched for; the rest is confirmed
  # with one comparison. byteindex of a whole ~400 KB fragment was the top
  # frame of a spliced request.
  PROBE_CHARS = 64
  # Constant text runs (between tokens) deflated once: bytes -> [the
  # run's canonical frozen copy, {predecessor => {gap => piece}}]. Keyed by
  # content, so a run is only ever reused for the same bytes; cleared when
  # full rather than tracked (a layout has a handful).
  @runs = {}
  RUN_ENTRIES = 256
  # Pieces kept per run, across predecessors and gaps.
  RUN_VARIANTS = 16
  # Runs shorter than this go out stored: a lookup hashes the run.
  RUN_MIN = 32

  # Called by the cache store for each fragment it hands to a view.
  def self.note_fragment(fragment)
    list = Thread.current[:rh_gzip_fragments]
    return if list.nil? || !fragment.is_a?(String) || !fragment.frozen?
    return if fragment.bytesize < SPLICE_MIN
    list << fragment
  end

  # One gzip member for `raw`, or nil when no recorded fragment is in it.
  def self.splice(raw, fragments, tokens = nil)
    return nil unless SPLICE_OK
    found = []
    pos = 0
    fragments.each do |frag|
      at = locate(raw, frag, pos)
      next if at.nil?
      found << at << frag
      pos = at + frag.bytesize
    end
    # Tokens alone are worth splicing only if one is in the body.
    return nil if found.empty? && token_cuts(raw, tokens).nil?

    # The text between fragments, as slices of `raw` (shared, not copied).
    texts = []
    frags = []
    cur = 0
    i = 0
    while i < found.length
      texts << raw.byteslice(cur, found[i] - cur)
      frags << found[i + 1]
      cur = found[i] + found[i + 1].bytesize
      i += 2
    end
    texts << raw.byteslice(cur, raw.bytesize - cur)

    last = @pieces_mutex.synchronize { @last_splice }
    if !last.nil? && same_splice?(last, frags, texts)
      # A repeated page: hand it to the last-hit path in `compress`, so the
      # next repeat is one comparison instead of a locate. Only a repeat
      # pays for this copy; a page with a per-request token never does.
      @mutex.synchronize do
        @last_raw = raw.dup
        @last_gz = last[2]
      end
      return last[2]
    end

    out = String.new(capacity: raw.bytesize / 6, encoding: Encoding::BINARY)
    out << GZIP_HEADER
    crc = 0
    pos = 0
    # `found` is [offset, frag, ...]; pair index `k` matches `frags[k]`.
    run_start = 0
    k = 0
    while k < frags.length
      text = texts[k]
      frag = frags[k]
      # Fold short glue into the next fragment only when it holds no
      # recorded request token. A token in the glue would be part of the
      # piece key, so every CSRF change would miss and re-deflate the
      # whole glue+fragment (and burn a PIECES_PER_FRAGMENT slot). Leave
      # token-bearing glue on splice_text (stored block) instead.
      if k > 0 && text.bytesize <= MAX_GLUE && token_cuts(text, tokens).nil?
        piece, piece_crc = fragment_piece(frag, text, found, k, run_start, pos, raw)
        out << piece
        crc = Zlib.crc32_combine(crc, piece_crc, text.bytesize + frag.bytesize)
        pos += text.bytesize + frag.bytesize
      else
        crc = splice_text(out, raw, pos, text, crc, tokens) unless text.empty?
        pos += text.bytesize
        run_start = pos
        piece, piece_crc = fragment_piece(frag, EMPTY, found, k, run_start, pos, raw)
        out << piece
        crc = Zlib.crc32_combine(crc, piece_crc, frag.bytesize)
        pos += frag.bytesize
      end
      k += 1
    end
    tail = texts[frags.length]
    crc = splice_text(out, raw, pos, tail, crc, tokens) unless tail.nil? || tail.empty?
    out << FINAL_BLOCK
    out << [crc, raw.bytesize & 0xffffffff].pack("VV")
    out.freeze
    # Texts are copied for keeping: the Rack body string may be reused and
    # mutated between requests (see compress), and they are small.
    @pieces_mutex.synchronize { @last_splice = [frags, texts.map { |t| t.dup.freeze }, out].freeze }
    out
  end

  # Where `frag` starts in `raw` at or after byte `pos`, or nil: its first
  # PROBE_CHARS characters are searched for, then the whole is compared.
  def self.locate(raw, frag, pos)
    probe = frag[0, PROBE_CHARS]
    at = raw.byteindex(probe, pos)
    until at.nil?
      return at if raw.byteslice(at, frag.bytesize) == frag
      at = raw.byteindex(probe, at + probe.bytesize)
    end
    nil
  rescue ArgumentError, IndexError, Encoding::CompatibilityError
    nil
  end

  def self.same_splice?(last, frags, texts)
    lf, lt = last
    return false unless lf.length == frags.length && lt.length == texts.length
    frags.each_with_index { |f, k| return false unless lf[k].equal?(f) }
    texts.each_with_index { |t, k| return false unless lt[k].bytesize == t.bytesize && lt[k] == t }
    true
  end

  # The per-request text `data`, starting at byte `pos`: fastest level, with the
  # body's real preceding bytes (up to the window) as its dictionary.
  def self.splice_text(out, raw, pos, data, crc, tokens = nil)
    cuts = data.bytesize < STORE_MAX ? nil : token_cuts(data, tokens)
    if data.bytesize < STORE_MAX
      stored(out, data)
    elsif cuts.nil?
      start = pos > WINDOW ? pos - WINDOW : 0
      dict = pos.zero? ? nil : raw.byteslice(start, pos - start)
      out << raw_deflate(data, dict, Zlib::BEST_SPEED)
    else
      at = 0
      i = 0
      prev = nil
      gap = 0
      while i < cuts.length
        s = cuts[i]
        n = cuts[i + 1]
        prev, gap = constant_run(out, data.byteslice(at, s - at), prev, gap) if s > at
        stored(out, data.byteslice(s, n))
        gap += n
        at = s + n
        i += 2
      end
      constant_run(out, data.byteslice(at, data.bytesize - at), prev, gap) if at < data.bytesize
    end
    Zlib.crc32_combine(crc, Zlib.crc32(data), data.bytesize)
  end

  # A stored block: BFINAL 0, BTYPE 00, padded to the byte boundary the
  # previous piece's SYNC_FLUSH left, then LEN, ~LEN and the bytes.
  def self.stored(out, data)
    n = data.bytesize
    out << [0, n, n ^ 0xffff].pack("Cvv") << data.b
  end

  # Where the request's tokens sit in `data`, as a flat sorted
  # [start, length, ...] of non-overlapping spans, or nil for none.
  def self.token_cuts(data, tokens)
    return nil if tokens.nil?
    spans = []
    tokens.each do |tok|
      at = data.byteindex(tok)
      until at.nil?
        spans << [at, tok.bytesize]
        at = data.byteindex(tok, at + tok.bytesize)
      end
    end
    return nil if spans.empty?
    spans.sort_by!(&:first)
    cuts = []
    last_end = 0
    spans.each do |s, n|
      next if s < last_end
      cuts << s << n
      last_end = s + n
    end
    cuts
  rescue ArgumentError, IndexError, Encoding::CompatibilityError
    nil
  end

  # A run of text with no token in it, deflated once per (run, predecessor,
  # gap) and reused by its bytes. Answers [predecessor, gap] for the next
  # run: this run's canonical copy and 0, or, for a run short enough to go
  # out stored, the predecessor kept and the gap grown by the run.
  #
  # The dictionary is the previous constant run followed by `gap` NULs:
  # the decompressor's window holds that run and then the gap's real bytes
  # (a token, which varies), so back-references into the run land at the
  # right distance. A reference into the gap would copy NULs, and a match
  # can only copy bytes the run itself holds, so a NUL-free run can never
  # refer into it; a run with a NUL, or a gap past the window, gets no
  # dictionary. Same bytes always come out of the same (run, predecessor,
  # gap), so the piece is reused for exactly those.
  def self.constant_run(out, run, prev = nil, gap = 0)
    if run.bytesize < RUN_MIN
      stored(out, run)
      return [prev, gap + run.bytesize]
    end
    prev = nil if gap >= WINDOW / 2 || run.include?("\0")
    entry = nil
    piece = nil
    @pieces_mutex.synchronize do
      entry = @runs[run]
      unless entry.nil?
        by_gap = entry[1][prev.nil? ? NO_PREV : prev]
        piece = by_gap[gap] unless by_gap.nil?
      end
    end
    if piece.nil?
      dict = nil
      unless prev.nil?
        keep = WINDOW - gap
        tail = prev.bytesize > keep ? prev.byteslice(prev.bytesize - keep, keep) : prev
        dict = tail.b + ("\0" * gap).b
      end
      piece = raw_deflate(run, dict, Zlib::DEFAULT_COMPRESSION).freeze
      @pieces_mutex.synchronize do
        entry = @runs[run]
        if entry.nil?
          @runs.clear if @runs.size >= RUN_ENTRIES
          entry = [run.dup.freeze, {}.compare_by_identity]
          @runs[entry[0]] = entry
        end
        variants = entry[1]
        variants.clear if variants.size >= RUN_VARIANTS
        (variants[prev.nil? ? NO_PREV : prev] ||= {})[gap] = piece
      end
    end
    out << piece
    [entry[0], 0]
  end
  NO_PREV = Object.new.freeze

  # Called with each masked CSRF token a request mints.
  def self.note_token(token)
    list = Thread.current[:rh_gzip_tokens]
    return if list.nil? || !token.is_a?(String) || token.empty?
    list << token
  end

  # A fragment's piece and CRC-32 for glue + frag, deflated against the
  # run's preceding bytes (up to WINDOW) as dictionary. `found` is the
  # flat [offset, frag, ...] locate built; `k` is this fragment's pair
  # index; `at` is where glue (or the fragment, when glue is empty) starts
  # in `raw`. Variants are keyed by predecessor chain + glue so a piece
  # compressed after A is never served after B (wrong-body refusal).
  def self.fragment_piece(frag, glue, found, k, run_start, at, raw)
    dict_start = at - WINDOW > run_start ? at - WINDOW : run_start
    # Without raw dictionaries every chain deflates the same; keep one
    # variant per glue (dict_start == at → empty chain, no dict).
    dict_start = at unless RAW_DICTIONARY_OK
    glue_key = glue.empty? ? EMPTY : glue
    hit = @pieces_mutex.synchronize do
      last = @last_piece
      if !last.nil? && last[0].equal?(frag) && last[1] == glue_key &&
          same_chain?(last[2][0], found, k, dict_start, raw)
        last[2]
      else
        found_e = nil
        list = @pieces[frag]
        unless list.nil?
          list.each do |e|
            next unless e[1] == glue_key && same_chain?(e[0], found, k, dict_start, raw)
            @last_piece = [frag, e[1], e].freeze
            found_e = e
            break
          end
        end
        found_e
      end
    end
    return [hit[2], hit[3]] unless hit.nil?

    data = glue.empty? ? frag : glue.b + frag
    dict = at > dict_start ? raw.byteslice(dict_start, at - dict_start) : nil
    deflated = raw_deflate(data, dict, Zlib::DEFAULT_COMPRESSION).freeze
    piece_crc = Zlib.crc32(data)
    chain = fragment_chain(found, k, dict_start, raw)
    glue_f = glue.empty? ? EMPTY : glue.b.freeze
    entry = [chain, glue_f, deflated, piece_crc].freeze
    @pieces_mutex.synchronize do
      list = @pieces[frag]
      if list.nil?
        list = []
        @pieces[frag] = list
      end
      list.shift if list.size >= PIECES_PER_FRAGMENT
      list << entry
      @last_piece = [frag, glue_f, entry].freeze
    end
    [deflated, piece_crc]
  end

  # [fragment, glue before it, fragment, glue, ...] going back from pair
  # `k` while those fragments end after `dict_start` — the identity the
  # dictionary depends on, after page_parts.rb.
  def self.fragment_chain(found, k, dict_start, raw)
    c = []
    j = k - 1
    while j >= 0
      off = found[j * 2]
      prev = found[j * 2 + 1]
      break if off + prev.bytesize <= dict_start
      prev_end = j > 0 ? found[(j - 1) * 2] + found[(j - 1) * 2 + 1].bytesize : off
      g = if off > prev_end && off > dict_start
        raw.byteslice(prev_end, off - prev_end).b.freeze
      else
        EMPTY
      end
      # WeakRef: a WeakKeyMap value must not strongly hold a fragment
      # that is also a map key (same fragment twice in a run, or a
      # predecessor that is itself cached). Strong refs would pin the
      # key after the fragment cache drops it.
      c << WeakRef.new(prev) << g
      j -= 1
    end
    c.freeze
  end

  def self.chain_pred_is?(stored, prev)
    return false unless stored.is_a?(WeakRef)
    stored.weakref_alive? && stored.__getobj__.equal?(prev)
  rescue WeakRef::RefError
    false
  end

  def self.same_chain?(chain, found, k, dict_start, raw)
    return false if chain.nil?
    i = 0
    j = k - 1
    while j >= 0
      off = found[j * 2]
      prev = found[j * 2 + 1]
      break if off + prev.bytesize <= dict_start
      return false if i >= chain.length || !chain_pred_is?(chain[i], prev)
      prev_end = j > 0 ? found[(j - 1) * 2] + found[(j - 1) * 2 + 1].bytesize : off
      glue_len = off > prev_end && off > dict_start ? off - prev_end : 0
      g = chain[i + 1]
      return false if g.bytesize != glue_len
      return false if glue_len > 0 && raw.byteslice(prev_end, glue_len) != g
      i += 2
      j -= 1
    end
    i == chain.length
  end

  # Raw deflate (no zlib or gzip framing), ending byte-aligned on a
  # non-final block, so pieces concatenate into one stream.
  def self.raw_deflate(data, dict, level)
    z = Zlib::Deflate.new(level, -Zlib::MAX_WBITS)
    z.set_dictionary(dict) if RAW_DICTIONARY_OK && !dict.nil?
    s = z.deflate(data, Zlib::SYNC_FLUSH)
    z.close
    s
  end

  # Prepended onto the overlay's cache store: every fragment a view gets
  # from `read_str` (a hit) or `write_str` (a miss, which returns the
  # stored frozen copy the view then appends) is noted for the splice.
  module Recorder
    def read_str(key)
      s = super
      GzipCache.note_fragment(s) unless s.nil?
      s
    end

    def write_str(key, value, ttl)
      s = super
      GzipCache.note_fragment(s)
      s
    end
  end

  def self.join_body(body)
    if body.is_a?(Array)
      n = body.length
      if n == 0
        body.close if body.respond_to?(:close)
        return ""
      end
      if n == 1
        s = body[0].to_s
        body.close if body.respond_to?(:close)
        return s
      end
    end
    parts = []
    body.each { |part| parts << part.to_s }
    body.close if body.respond_to?(:close)
    parts.join
  end

  def self.header(headers, name)
    headers[name] || headers[name.split("-").map(&:capitalize).join("-")]
  end

  def self.accepts_gzip?(accept)
    accept.to_s.downcase.split(",").any? { |part|
      coding, *params = part.strip.split(";")
      next false unless coding == "gzip" || coding == "x-gzip"
      q = "1"
      params.each { |p|
        k, v = p.strip.split("=", 2)
        q = v.to_s if k == "q"
      }
      q.to_f > 0.0
    }
  end

  def self.binary?(ct)
    return false if ct.nil? || ct.empty?
    ct.start_with?("image/") || ct.start_with?("audio/") ||
      ct.start_with?("video/") || ct.start_with?("font/") ||
      ct.start_with?("application/octet-stream") ||
      ct.start_with?("application/zip") ||
      ct.start_with?("application/gzip") ||
      ct.start_with?("application/wasm")
  end
end

Rails::MemoryStore.prepend(GzipCache::Recorder) if defined?(Rails::MemoryStore)

if defined?(ActionController::AuthenticityToken)
  module GzipCache
    module TokenRecorder
      def masked
        tok = super
        GzipCache.note_token(tok)
        tok
      end
    end
  end
  ActionController::AuthenticityToken.singleton_class.prepend(GzipCache::TokenRecorder)
end
