# `multipart/form-data` for the ruby family: the parser, and the
# `ActionDispatch::Http::UploadedFile` a file part becomes.
#
# Rack does this for a Rails app and hands the controller a
# `Rack::Multipart::UploadedFile`-shaped Hash; here both serving paths
# — tep (`Tep::Request#consume_body`) and the CGI/Rack shim
# (`CgiIo.parse_request`) — call `Multipart.parse` and put each file
# part into the params tree as an `UploadedFile` object, under the same
# nested key a text field would take (`params["message"]["attachment"]`).
# The synthesized `<Resource>Params.from_raw` reads it back through
# `UploadedFile.from_params`, which is the ONE place a params value is
# narrowed to this class: `Roundhouse::ParamValue` (String | Hash |
# Array) is what every target's params tree carries, and this class is
# a fourth arm only the ruby family's untyped Hash can hold. That is
# why none of this is in `runtime/ruby/`.
#
# The body is BYTES. Every offset here is a byte offset (`byteindex`,
# `byteslice`, `bytesize`): a JPEG in the body is not valid UTF-8, and
# a character index over it is undefined on CRuby and wrong on spinel.
#
# What is read: `Content-Disposition: form-data; name="…"; filename=
# "…"` and the part's `Content-Type`. A part with a `filename` is a
# file; without one, a text field. A file part whose filename is EMPTY
# is what a browser sends for a file input nobody picked — Rack drops
# it, and so does this (the field is simply not provided, which is
# what `Params.provided` then says).
#
# Not modeled: nested multipart (`multipart/mixed` inside a part),
# `Content-Transfer-Encoding`, and RFC 2231 encoded filenames. Rack
# reads none of the first two for a browser form either.
module ActionDispatch
  module Http
    # What `fixture_file_upload` hands a test and what a multipart part
    # becomes in production: the name it was uploaded under, its
    # declared type, and its bytes. Data-backed rather than file-backed
    # so a part never touches the disk before the storage service
    # decides where it lives.
    class UploadedFile
      attr_reader :original_filename, :content_type

      def initialize(data, original_filename, content_type)
        @data = data
        @original_filename = original_filename
        @content_type = content_type
      end

      def read
        @data
      end

      def size
        @data.bytesize
      end

      # A params hash carrying one stringifies to the uploaded name,
      # which is what Rails' own `to_s` gives.
      def to_s
        @original_filename
      end

      # The file under `key` in a params sub-hash, or nil when the
      # request carried no file there (absent, or a text field — the
      # bare filename a urlencoded form posts for a file input is a
      # String, not a file).
      def self.from_params(sub, key)
        value = sub[key]
        if value.is_a?(ActionDispatch::Http::UploadedFile)
          value
        else
          nil
        end
      end

      # Did the request carry a file under `key`? The file twin of
      # `Params.provided`.
      def self.provided(sub, key)
        !from_params(sub, key).nil?
      end

      # `to_h` on a params record: the uploaded name, or "" for none.
      def self.name_of(file)
        file.nil? ? "" : file.original_filename
      end
    end

    module Multipart
      # The parsed body: text fields by full name (`message[body]`),
      # files by full name. Two typed hashes rather than one mixed one,
      # so each serving path can nest them into its own params shape.
      class Form
        attr_reader :fields, :files

        def initialize
          @fields = {}
          @files = {}
        end

        def add_field(name, value)
          @fields[name] = value
          nil
        end

        def add_file(name, file)
          @files[name] = file
          nil
        end
      end

      # The boundary parameter of a `multipart/form-data; boundary=…`
      # header, unquoted; "" when the header has none.
      def self.boundary(content_type)
        at = content_type.index("boundary=")
        return "" if at.nil?
        rest = content_type[at + 9, content_type.length - at - 9].to_s
        semi = rest.index(";")
        rest = rest[0, semi].to_s unless semi.nil?
        rest = rest.strip
        if rest.length >= 2 && rest[0, 1] == "\"" && rest[rest.length - 1, 1] == "\""
          rest = rest[1, rest.length - 2].to_s
        end
        rest
      end

      # The value of `<attr>="…"` inside a Content-Disposition header,
      # or "" when absent.
      def self.disposition_param(header, attr)
        needle = attr + "=\""
        at = header.index(needle)
        return "" if at.nil?
        from = at + needle.length
        close = header.index("\"", from)
        return "" if close.nil?
        header[from, close - from].to_s
      end

      def self.parse(body, content_type)
        form = Form.new
        b = boundary(content_type)
        return form if b.length == 0
        delimiter = "--" + b
        n = body.bytesize
        pos = body.byteindex(delimiter)
        return form if pos.nil?
        pos += delimiter.bytesize
        while pos < n
          # The delimiter is followed by CRLF for a part, or by "--"
          # for the closing delimiter.
          return form if body.byteslice(pos, 2) == "--"
          pos += 2 if body.byteslice(pos, 2) == "\r\n"
          header_end = body.byteindex("\r\n\r\n", pos)
          return form if header_end.nil?
          headers = body.byteslice(pos, header_end - pos).to_s
          content_start = header_end + 4
          next_delim = body.byteindex("\r\n" + delimiter, content_start)
          return form if next_delim.nil?
          content = body.byteslice(content_start, next_delim - content_start).to_s
          add_part(form, headers, content)
          pos = next_delim + 2 + delimiter.bytesize
        end
        form
      end

      # One part: its name, whether it is a file, and its bytes. The
      # header block is small and ASCII, so character ops are byte ops
      # there.
      def self.add_part(form, headers, content)
        disposition = +""
        part_type = "application/octet-stream"
        headers.split("\r\n").each do |line|
          colon = line.index(":")
          next if colon.nil?
          hname = line[0, colon].to_s.strip.downcase
          hvalue = line[colon + 1, line.length - colon - 1].to_s.strip
          if hname == "content-disposition"
            disposition = hvalue
          elsif hname == "content-type"
            part_type = hvalue
          end
        end
        name = disposition_param(disposition, "name")
        return nil if name.length == 0
        if disposition.include?("filename=")
          filename = disposition_param(disposition, "filename")
          return nil if filename.length == 0
          # A browser sends the basename; a client that sends a path
          # gets the same treatment Rack gives it.
          slash = filename.rindex("/")
          filename = filename[slash + 1, filename.length - slash - 1].to_s unless slash.nil?
          bslash = filename.rindex("\\")
          filename = filename[bslash + 1, filename.length - bslash - 1].to_s unless bslash.nil?
          form.add_file(name, UploadedFile.new(content, filename, part_type))
        else
          form.add_field(name, content)
        end
        nil
      end
    end
  end
end
