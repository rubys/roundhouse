# `Rack::Utils.q_values` and `Rack::Utils.select_best_encoding`, ported
# from rack 3.2 (lib/rack/utils.rb) for the trees that have no rack gem
# to load. The ruby family swaps this file for `require "rack/utils"`
# (see `project::ruby_runtime_files`), so the gem's own code runs there.
#
# campfire's `CachedResponses#cache_read_response` negotiates the body's
# encoding with them before it reuses a cached page:
#
#     Rack::Utils.select_best_encoding(%w[ gzip identity ],
#       Rack::Utils.q_values(request.headers["Accept-Encoding"]))
#
# so the answer decides whether a client is sent gzip bytes, and has to
# be rack's exactly — including `identity` being acceptable unless a
# client refuses it with `q=0`, and a `*` standing for every available
# encoding the header did not name.
#
# The two methods are rack's, with its multiple assignments and
# `&:first` spelled as index reads and its sort written out. `q_values` answers a
# pair per comma-separated part: the media/encoding name (nil for an
# empty part, as rack's destructuring of `[]` gives) and its quality.
module Rack
  module Utils
    def self.q_values(q_value_header)
      out = []
      q_value_header.to_s.split(",").each do |part|
        pieces = part.split(";", 2)
        value = pieces.empty? ? nil : pieces[0].strip
        quality = 1.0
        if pieces.length > 1
          md = /\Aq=([\d.]+)/.match(pieces[1].strip)
          quality = md[1].to_f unless md.nil?
        end
        out.push([value, quality])
      end
      out
    end

    # Only the first 16 encodings are considered, as rack does.
    def self.select_best_encoding(available_encodings, accept_encoding)
      accept = accept_encoding.take(16)
      expanded = []
      wildcard_seen = false
      accept.each do |pair|
        m = pair[0]
        q = pair[1]
        preference = available_encodings.index(m) || available_encodings.size
        if m == "*"
          unless wildcard_seen
            named = accept.map { |named_pair| named_pair[0] }
            (available_encodings - named).each do |m2|
              expanded.push([m2, q, preference])
            end
            wildcard_seen = true
          end
        else
          expanded.push([m, q, preference])
        end
      end
      # Rack sorts by quality, highest first, then by the server's
      # preference. A stable insertion sort with that order: entries that
      # tie on both are the same name, or names not on offer, so the
      # answer does not depend on how a tie is broken.
      # `take(0)`, not `[]`: an empty Array of `expanded`'s own entry type.
      sorted = expanded.take(0)
      expanded.each do |entry|
        i = 0
        while i < sorted.length && (sorted[i][1] > entry[1] || (sorted[i][1] == entry[1] && sorted[i][2] <= entry[2]))
          i += 1
        end
        sorted.insert(i, entry)
      end
      candidates = sorted.map { |entry| entry[0] }
      candidates.push("identity") unless candidates.include?("identity")
      expanded.each do |entry|
        candidates.delete(entry[0]) if entry[1] == 0.0
      end
      (candidates & available_encodings)[0]
    end
  end
end
