# frozen_string_literal: true
# Rails 8.1.4 `send_data(data, type:, disposition:, filename:)`.
#
# With a filename the Content-Disposition header is what
# `ActionDispatch::Http::ContentDisposition.format` writes:
# `attachment; filename="a.tgz"; filename*=UTF-8''a.tgz`.
#
# Ruby family and Spinel only. The escaping works over the name's UTF-8
# bytes, and the shared runtime's String is code points on the strict
# targets, so they keep the shared `send_data` (no `filename:`) in
# runtime/ruby/action_controller/base.rb.
module ActionController
  class Base
    def send_data(data, type: "application/octet-stream", disposition: "attachment", filename: nil)
      @_body = data
      @_content_type = type
      @_performed = true
      disp = disposition.to_s == "inline" ? "inline" : "attachment"
      @_headers["Content-Disposition"] = filename.nil? ? disp : ActionController::Base.content_disposition(disp, filename)
      apply_vary_header
      nil
    end

    # Rails' two escaping sets: the quoted name keeps the space and
    # A-Za-z0-9 ! # $ + - . ^ _ ` | ~ and percent-escapes every other
    # byte; the RFC 5987 name keeps the same set plus `&`, and escapes the
    # space. One divergence: Rails transliterates a non-ASCII character
    # for the quoted name through I18n (`z` for `ż`); this writes `%3F`
    # (`?`) per character. `filename*`, which browsers read, matches
    # Rails byte for byte.
    def self.content_disposition(disp, filename)
      name = filename.to_s
      disp + "; filename=\"" + content_disposition_ascii(name) +
        "\"; filename*=UTF-8''" + content_disposition_utf8(name)
    end

    # One `%3F` per non-ASCII character: a UTF-8 lead byte (0xC0 and up)
    # writes it, a continuation byte (0x80..0xBF) writes nothing.
    def self.content_disposition_ascii(name)
      out = +""
      name.bytes.each do |b|
        if b >= 192
          out << "%3F"
        elsif b >= 128
          nil
        elsif b == 32 || content_disposition_plain?(b)
          out << b.chr
        else
          out << format("%%%02X", b)
        end
      end
      out
    end

    def self.content_disposition_utf8(name)
      out = +""
      name.bytes.each do |b|
        if b == 38 || content_disposition_plain?(b)
          out << b.chr
        else
          out << format("%%%02X", b)
        end
      end
      out
    end

    # A-Z a-z 0-9 and ! # $ + - . ^ _ ` | ~
    def self.content_disposition_plain?(b)
      (b >= 48 && b <= 57) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122) ||
        b == 33 || b == 35 || b == 36 || b == 43 || b == 45 || b == 46 ||
        b == 94 || b == 95 || b == 96 || b == 124 || b == 126
    end
  end
end
