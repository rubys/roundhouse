# Spinel-only CGI shim. CRuby/JRuby use stdlib `require "cgi"`; the AOT tree
# has no stdlib CGI, so provide the pure-function subset lobsters reaches.
#
# Hot path: front-page story archive links render `CGI.escape(story.url)` per
# story (stories/_listdetail), routed to the tep Url percent-encoder.
# `unescape_html` (extras/html_encoder) and `parse` (extras/github OAuth) are
# off the benchmark path — real, but not tuned.
#
# NOT NAMED `cgi.rb`: this shim substitutes for a stdlib library on
# spinel ONLY — the CRuby/JRuby boot reaches the real one with a bare
# `require "cgi"` (ruby_overlay/boot.rb). `walk_dir_flat` copies every
# runtime/spinel/*.rb into EVERY target tree, so under the library's
# own name this file would answer that bare require whenever
# `runtime/` is on `$LOAD_PATH` — which the emitted Rakefile does
# (`t.libs << "runtime"`) — handing CRuby the AOT subset instead of
# the stdlib. See runtime/json_impl.rb for the same rule.
module CGI
  def self.escape(s)
    Url.escape(s)
  end

  # Decode the named entities lobsters content carries (`&amp;`/`&lt;`/`&gt;`/
  # `&quot;`/`&apos;`) plus numeric `&#N;` (ASCII). Unknown entities pass
  # through verbatim, as Rails' CGI does. Regex-free scan (spinel-safe).
  def self.unescape_html(s)
    out = +""
    i = 0
    n = s.length
    while i < n
      c = s[i]
      if c == "&"
        # locate the terminating ';' within a short window
        j = i + 1
        semi = -1
        while j < n && j - i <= 12
          if s[j] == ";"
            semi = j
            j = n
          else
            j += 1
          end
        end
        rep = ""
        if semi >= 0
          d = CGI.decode_entity(s[(i + 1), semi - i - 1])
          rep = d unless d.nil?
        end
        if rep.length == 0
          out << c
          i += 1
        else
          out << rep
          i = semi + 1
        end
      else
        out << c
        i += 1
      end
    end
    out
  end

  def self.decode_entity(ent)
    return "&" if ent == "amp"
    return "<" if ent == "lt"
    return ">" if ent == "gt"
    return "\"" if ent == "quot"
    return "'" if ent == "apos"
    if ent.length > 1 && ent[0, 1] == "#"
      num = ent[1, ent.length - 1].to_i
      return num.chr if num > 0 && num < 128
    end
    nil
  end

  # `CGI.parse(qs)` -> key -> [values] (the CGI contract; extras/github reads
  # `ps["access_token"].first`). Off-bench; values URL-decoded.
  def self.parse(qs)
    out = {}
    pairs = qs.split("&")
    i = 0
    while i < pairs.length
      pair = pairs[i]
      eq = pair.index("=")
      unless eq.nil?
        k = Url.unescape(pair[0, eq])
        v = Url.unescape(pair[(eq + 1), pair.length - eq - 1])
        out[k] = [v]
      end
      i += 1
    end
    out
  end
end
