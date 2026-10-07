# frozen_string_literal: true

# CgiIo's URL codec on the C implementations CRuby already ships.
#
# runtime/cgi_io.rb walks bytes, which is what Spinel compiles. Under an
# interpreter that walk is the slow part: `url_encode` allocated a String
# per byte (`b.chr`) and called `format` per escape, and `url_decode`
# loops over the session cookie, which always carries escapes, on every
# request. `CGI.escapeURIComponent` keeps exactly the unreserved set
# (A-Z a-z 0-9 - . _ ~) and writes uppercase %XX for every other byte,
# as `url_encode` does; `CGI.unescape` maps `+` to space, decodes %XX and
# leaves a malformed escape alone, as `url_decode` does. The decode fast
# path (nothing to decode: the input itself) is kept as it was.
#
# Loaded after runtime/cgi_io by boot.rb. A runtime without these (an
# older cgi/escape) keeps the portable methods.
begin
  require "cgi/escape"
rescue LoadError
end

if defined?(CGI) && CGI.respond_to?(:escapeURIComponent) && CGI.respond_to?(:unescape)
  module CgiIo
    def self.url_encode(s)
      CGI.escapeURIComponent(s.to_s)
    end

    def self.url_decode(s)
      return s unless s.include?("%") || s.include?("+")
      CGI.unescape(s, Encoding::UTF_8)
    end
  end
end
