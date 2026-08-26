# `Mime::Type` — the slice of actionpack's MIME registry the corpus
# reaches, ported rather than derived.
#
# The three tables below are GENERATED from actionpack 8.1.3's own
# `action_dispatch/http/mime_types.rb`, by running that file against a
# recording `Mime::Type.register`. Deriving the mapping by hand gets it
# subtly wrong: `register "text/vtt", :vtt, %w(vtt)` passes "vtt" in the
# MIME-synonym position, not the extension one, so `lookup("vtt")`
# really does answer the vtt type upstream — a quirk a from-scratch
# table would have "fixed" into a divergence. Re-port, don't edit, when
# the pinned version moves.
#
# Only `lookup`, `lookup_by_extension` and `Mime.[]` are implemented;
# a call beyond them stays an honest gap rather than a method that
# types and then raises. Absent on purpose: `register` (the table is
# frozen at port time, so an app cannot add to it), `parse` and the
# Accept-header machinery, `SET`/`symbols`, and the `===`/`=~`
# comparison surface.
module Mime
  class Type
    class InvalidMimeType < StandardError; end

    CANONICAL = {
          "text/html" => "text/html",
          "application/xhtml+xml" => "text/html",
          "text/plain" => "text/plain",
          "text/javascript" => "text/javascript",
          "application/javascript" => "text/javascript",
          "application/x-javascript" => "text/javascript",
          "text/css" => "text/css",
          "text/calendar" => "text/calendar",
          "text/csv" => "text/csv",
          "text/vcard" => "text/vcard",
          "text/vtt" => "text/vtt",
          "vtt" => "text/vtt",
          "text/markdown" => "text/markdown",
          "image/png" => "image/png",
          "image/jpeg" => "image/jpeg",
          "image/gif" => "image/gif",
          "image/bmp" => "image/bmp",
          "image/tiff" => "image/tiff",
          "image/svg+xml" => "image/svg+xml",
          "image/webp" => "image/webp",
          "video/mpeg" => "video/mpeg",
          "audio/mpeg" => "audio/mpeg",
          "audio/ogg" => "audio/ogg",
          "audio/aac" => "audio/aac",
          "audio/mp4" => "audio/aac",
          "video/webm" => "video/webm",
          "video/mp4" => "video/mp4",
          "font/otf" => "font/otf",
          "font/ttf" => "font/ttf",
          "font/woff" => "font/woff",
          "font/woff2" => "font/woff2",
          "application/xml" => "application/xml",
          "text/xml" => "application/xml",
          "application/x-xml" => "application/xml",
          "application/rss+xml" => "application/rss+xml",
          "application/atom+xml" => "application/atom+xml",
          "application/x-yaml" => "application/x-yaml",
          "text/yaml" => "application/x-yaml",
          "multipart/form-data" => "multipart/form-data",
          "application/x-www-form-urlencoded" => "application/x-www-form-urlencoded",
          "application/json" => "application/json",
          "text/x-json" => "application/json",
          "application/jsonrequest" => "application/json",
          "application/problem+json" => "application/json",
          "application/pdf" => "application/pdf",
          "application/zip" => "application/zip",
          "application/gzip" => "application/gzip",
          "application/x-gzip" => "application/gzip",
    }.freeze

    SYMBOLS = {
          "text/html" => :html,
          "text/plain" => :text,
          "text/javascript" => :js,
          "text/css" => :css,
          "text/calendar" => :ics,
          "text/csv" => :csv,
          "text/vcard" => :vcf,
          "text/vtt" => :vtt,
          "text/markdown" => :md,
          "image/png" => :png,
          "image/jpeg" => :jpeg,
          "image/gif" => :gif,
          "image/bmp" => :bmp,
          "image/tiff" => :tiff,
          "image/svg+xml" => :svg,
          "image/webp" => :webp,
          "video/mpeg" => :mpeg,
          "audio/mpeg" => :mp3,
          "audio/ogg" => :ogg,
          "audio/aac" => :m4a,
          "video/webm" => :webm,
          "video/mp4" => :mp4,
          "font/otf" => :otf,
          "font/ttf" => :ttf,
          "font/woff" => :woff,
          "font/woff2" => :woff2,
          "application/xml" => :xml,
          "application/rss+xml" => :rss,
          "application/atom+xml" => :atom,
          "application/x-yaml" => :yaml,
          "multipart/form-data" => :multipart_form,
          "application/x-www-form-urlencoded" => :url_encoded_form,
          "application/json" => :json,
          "application/pdf" => :pdf,
          "application/zip" => :zip,
          "application/gzip" => :gzip,
    }.freeze

    EXTENSIONS = {
          "html" => "text/html",
          "xhtml" => "text/html",
          "text" => "text/plain",
          "txt" => "text/plain",
          "js" => "text/javascript",
          "css" => "text/css",
          "ics" => "text/calendar",
          "csv" => "text/csv",
          "vcf" => "text/vcard",
          "vtt" => "text/vtt",
          "md" => "text/markdown",
          "markdown" => "text/markdown",
          "png" => "image/png",
          "jpeg" => "image/jpeg",
          "jpg" => "image/jpeg",
          "jpe" => "image/jpeg",
          "pjpeg" => "image/jpeg",
          "gif" => "image/gif",
          "bmp" => "image/bmp",
          "tiff" => "image/tiff",
          "tif" => "image/tiff",
          "svg" => "image/svg+xml",
          "webp" => "image/webp",
          "mpeg" => "video/mpeg",
          "mpg" => "video/mpeg",
          "mpe" => "video/mpeg",
          "mp3" => "audio/mpeg",
          "mp1" => "audio/mpeg",
          "mp2" => "audio/mpeg",
          "ogg" => "audio/ogg",
          "oga" => "audio/ogg",
          "spx" => "audio/ogg",
          "opus" => "audio/ogg",
          "m4a" => "audio/aac",
          "mpg4" => "audio/aac",
          "aac" => "audio/aac",
          "webm" => "video/webm",
          "mp4" => "video/mp4",
          "m4v" => "video/mp4",
          "otf" => "font/otf",
          "ttf" => "font/ttf",
          "woff" => "font/woff",
          "woff2" => "font/woff2",
          "xml" => "application/xml",
          "rss" => "application/rss+xml",
          "atom" => "application/atom+xml",
          "yaml" => "application/x-yaml",
          "yml" => "application/x-yaml",
          "multipart_form" => "multipart/form-data",
          "url_encoded_form" => "application/x-www-form-urlencoded",
          "json" => "application/json",
          "pdf" => "application/pdf",
          "zip" => "application/zip",
          "gzip" => "application/gzip",
          "gz" => "application/gzip",
    }.freeze

    attr_reader :symbol

    def initialize(string, symbol = nil)
      @string = string
      @symbol = symbol
    end

    def to_s
      @string
    end

    def to_str
      @string
    end

    def to_sym
      @symbol
    end

    def ==(other)
      other.is_a?(Mime::Type) && to_s == other.to_s
    end

    def eql?(other)
      self == other
    end

    def hash
      @string.hash
    end

    def inspect
      "#<Mime::Type:#{@string}>"
    end

    # Upstream's contract, which callers lean on: this NEVER answers nil.
    # An unregistered-but-well-formed string comes back as a Type whose
    # `symbol` is nil, which is why `if type = Mime::Type.lookup(ct)` is
    # a safe guard in app code and why the signature is non-nilable here.
    def self.lookup(string)
      canonical = CANONICAL[string]
      if canonical.nil?
        # Fall back to the media type without its parameters, so
        # "text/html; charset=utf-8" finds "text/html".
        head = string.split(";", 2)[0]
        base = head.nil? ? "" : head.rstrip
        canonical = CANONICAL[base]
        return new(canonical, SYMBOLS[canonical]) unless canonical.nil?
        raise InvalidMimeType, "#{string.inspect} is not a valid MIME type" unless valid?(base)
        return new(base, nil)
      end
      new(canonical, SYMBOLS[canonical])
    end

    def self.lookup_by_extension(extension)
      canonical = EXTENSIONS[extension.to_s]
      canonical.nil? ? nil : new(canonical, SYMBOLS[canonical])
    end

    # Structural, not RFC-exact. Upstream validates with a single
    # possessive-quantifier regexp (`MIME_REGEXP`) that does not port to
    # the strict targets; this accepts `type/subtype` with non-empty,
    # separator-free halves and rejects the rest. The divergence is
    # narrower than it looks — every registered type takes the table
    # path above and never reaches here — but a pathological subtype
    # upstream would reject can pass here.
    def self.valid?(string)
      return false if string.nil? || string == ""
      return true if string == "*/*"
      parts = string.split("/")
      return false unless parts.length == 2
      left = parts[0]
      right = parts[1]
      return false if left.nil? || left == "" || right.nil? || right == ""
      return false if left.include?(" ") || right.include?(" ")
      true
    end
  end

  def self.[](extension)
    Mime::Type.lookup_by_extension(extension)
  end
end
