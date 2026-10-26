# frozen_string_literal: true
# ActiveSupport::JSON.encode without the jsonable walk when it would
# change nothing. The shared encoder copies the value into JSON-native
# shapes first (Symbols to Strings, Times to Rails' format, a non-finite
# Float to nil), then generates. Most values handed to it already are
# native: campfire's fragment keys digest record attributes, which the
# emit keeps as their stored Strings and Integers. `JSON.generate(value,
# strict: true)` encodes exactly those (and Symbols as their names, which
# is what the walk makes of them) and raises on everything else, a Time,
# NaN, a non-String key, or any subclass of String or Hash, which then
# takes the shared walk as before. The same bytes either way.
#
# A campfire fragment key on rubix2 (2026-10-22): 5.85 -> 4.55 us, and
# with `escape: false` (encode_without_escape) 0.77.
require "json"

module ActiveSupport
  module JSON
    class << self
      def encode(value)
        JsonBuilder.escape_html_entities(encode_without_escape(value))
      end

      def encode_without_escape(value)
        ::JSON.generate(value, strict: true)
      rescue ::JSON::GeneratorError
        ::JSON.generate(ActiveSupport::JSON.jsonable(value))
      end
    end
  end
end
