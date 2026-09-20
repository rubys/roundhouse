require "json"
# Active Storage's BYTES half, for the ruby family: the disk service,
# the attachable coercion, the image analyzer, and the three engine
# routes that serve a blob back. Reopens the shared
# `runtime/ruby/active_storage.rb`, whose own header says why each of
# these is a per-target seam — every one of them reads or writes a file,
# narrows an untyped value by class, or reads bytes, none of which the
# shared runtime does. Loaded from both boot.rb files (spinel and the
# CRuby overlay), after action_controller so the `polymorphic_url`
# reopen below wins that file's raising definition.
#
# The layout is Rails' own `ActiveStorage::Service::DiskService`:
# `<root>/<key[0,2]>/<key[2,2]>/<key>`, so a `storage/` volume this
# tree wrote can be read by a Rails process pointed at the same
# database, and vice versa. `root` is `storage/files` (campfire's
# production `config/storage.yml`) and `tmp/storage` under RAILS_ENV=
# test, as Rails' generated storage.yml has it — the test harness cleans
# `tmp/storage` between files rather than between tests, which is also
# what Rails does.
require "digest"

module ActiveStorage
  class Service
    def root
      Rails.env_name == "test" ? "tmp/storage" : "storage/files"
    end

    def path_for(key)
      root + "/" + key[0, 2].to_s + "/" + key[2, 2].to_s + "/" + key
    end

    # `mkdir -p` for the two-level fan-out, one segment at a time:
    # `Dir.mkdir` makes one directory and the ruby family's subset has
    # no `FileUtils`.
    def ensure_dir(path)
      built = +""
      path.split("/").each do |seg|
        next if seg.length == 0
        built = built.length == 0 ? seg : built + "/" + seg
        Dir.mkdir(built) unless Dir.exist?(built)
      end
      nil
    end

    def upload(key, data)
      ensure_dir(root + "/" + key[0, 2].to_s + "/" + key[2, 2].to_s)
      File.binwrite(path_for(key), data)
      nil
    end

    def download(key)
      File.binread(path_for(key))
    end

    def delete(key)
      path = path_for(key)
      File.delete(path) if File.exist?(path)
      nil
    end

    def exist?(key)
      File.exist?(path_for(key))
    end
  end

  class Blob
    # Rails' attachable coercion: an `ActionDispatch::Http::UploadedFile`
    # (a multipart part, or the test harness's fixture) becomes a blob
    # by uploading it; a blob is itself; a String is a signed blob id
    # from a direct upload. Anything else — including the bare filename
    # a urlencoded form posts for a file field — attaches nothing.
    def self.from_attachable(value)
      if value.is_a?(ActionDispatch::Http::UploadedFile)
        Blob.create_and_upload!(value.read, value.original_filename, value.content_type)
      elsif value.is_a?(ActiveStorage::Blob)
        value
      elsif value.is_a?(String)
        Blob.find_signed(value)
      else
        nil
      end
    end
  end

  # Width and height from the file's header, for the formats a browser
  # renders inline: PNG, GIF, JPEG, BMP, WebP. Header reads only — no
  # decoder — which is all Rails' own `ImageAnalyzer` reports either.
  # `[0, 0]` for anything else, which `BlobMetadata` answers as nil.
  class ImageAnalyzer
    def self.dimensions(data, content_type)
      n = data.bytesize
      return [0, 0] if n < 10
      b0 = data.getbyte(0).to_i
      b1 = data.getbyte(1).to_i
      if b0 == 0x89 && b1 == 0x50 && n >= 24
        return [be32(data, 16), be32(data, 20)]
      end
      if b0 == 0x47 && b1 == 0x49
        return [le16(data, 6), le16(data, 8)]
      end
      if b0 == 0x42 && b1 == 0x4D && n >= 26
        h = le32(data, 22)
        h = 4294967296 - h if h > 2147483647
        return [le32(data, 18), h]
      end
      if b0 == 0xFF && b1 == 0xD8
        return jpeg_dimensions(data)
      end
      if b0 == 0x52 && b1 == 0x49 && n >= 30 && data.byteslice(8, 4) == "WEBP"
        return webp_dimensions(data)
      end
      [0, 0]
    end

    def self.be16(data, at)
      data.getbyte(at).to_i * 256 + data.getbyte(at + 1).to_i
    end

    def self.be32(data, at)
      be16(data, at) * 65536 + be16(data, at + 2)
    end

    def self.le16(data, at)
      data.getbyte(at + 1).to_i * 256 + data.getbyte(at).to_i
    end

    def self.le24(data, at)
      data.getbyte(at + 2).to_i * 65536 + le16(data, at)
    end

    def self.le32(data, at)
      le16(data, at + 2) * 65536 + le16(data, at)
    end

    # Walk the marker segments to the first SOFn frame header, which
    # carries the dimensions. Stand-alone markers (SOI, EOI, RSTn, TEM)
    # have no length; everything else is `FF xx <len16>`.
    def self.jpeg_dimensions(data)
      n = data.bytesize
      i = 2
      while i + 9 < n
        return [0, 0] if data.getbyte(i).to_i != 0xFF
        marker = data.getbyte(i + 1).to_i
        if marker == 0xFF
          i += 1
          next
        end
        if marker == 0xD8 || marker == 0xD9 || marker == 0x01 || (marker >= 0xD0 && marker <= 0xD7)
          i += 2
          next
        end
        sof = (marker >= 0xC0 && marker <= 0xC3) || (marker >= 0xC5 && marker <= 0xC7) ||
              (marker >= 0xC9 && marker <= 0xCB) || (marker >= 0xCD && marker <= 0xCF)
        if sof
          return [be16(data, i + 7), be16(data, i + 5)]
        end
        i += 2 + be16(data, i + 2)
      end
      [0, 0]
    end

    # The three WebP container flavours, each with the size in its own
    # place: lossy (VP8 ), lossless (VP8L, 14-bit fields), extended
    # (VP8X, 24-bit fields minus one).
    def self.webp_dimensions(data)
      kind = data.byteslice(12, 4).to_s
      if kind == "VP8 "
        return [le16(data, 26) & 0x3FFF, le16(data, 28) & 0x3FFF]
      end
      if kind == "VP8L"
        bits = le32(data, 21)
        return [(bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1]
      end
      if kind == "VP8X"
        return [le24(data, 24) + 1, le24(data, 27) + 1]
      end
      [0, 0]
    end
  end

  # The engine's routes, mounted by the dispatcher beside the app's
  # own table (`Main.route_table`), and the controllers behind them.
  # Rails answers the two redirect routes with a redirect to the disk
  # route — the blob's SERVICE URL, which for the disk service is this
  # same process — so that is what these do. The last two are the
  # DIRECT UPLOAD pair: the browser POSTs the blob's metadata and gets
  # a signed PUT url back, then PUTs the bytes there.
  #
  # `*filename` is the glob Rails puts last: cosmetic (the key travels
  # in the signed segment), so the router's own `.ext` format peel
  # (`moon.jpg` → `moon` + `format=jpg`) costs nothing here.
  module Routes
    def self.table
      [
        ActionDispatch::Router::Route.new(
          "GET", "/rails/active_storage/blobs/redirect/:signed_id/*filename",
          :active_storage_blobs_redirect, :show
        ),
        ActionDispatch::Router::Route.new(
          "GET", "/rails/active_storage/representations/redirect/:signed_blob_id/:variation_key/*filename",
          :active_storage_representations_redirect, :show
        ),
        ActionDispatch::Router::Route.new(
          "GET", "/rails/active_storage/disk/:encoded_key/*filename",
          :active_storage_disk, :show
        ),
        ActionDispatch::Router::Route.new(
          "POST", "/rails/active_storage/direct_uploads",
          :active_storage_direct_uploads, :create
        ),
        ActionDispatch::Router::Route.new(
          "PUT", "/rails/active_storage/disk/:encoded_token",
          :active_storage_disk, :update
        ),
      ]
    end

    # The dispatcher's `instantiate_controller` falls through to this
    # for a symbol its own (app-generated) table does not name.
    def self.instantiate_controller(sym)
      if sym == :active_storage_blobs_redirect
        ActiveStorage::Blobs::RedirectController.new
      elsif sym == :active_storage_representations_redirect
        ActiveStorage::Representations::RedirectController.new
      elsif sym == :active_storage_direct_uploads
        ActiveStorage::DirectUploadsController.new
      else
        ActiveStorage::DiskController.new
      end
    end
  end

  # Rails' `ActiveStorage::SetCurrent`, which every engine controller
  # includes: the request's protocol and host become the options the
  # disk service builds its urls against, so a redirect or a direct
  # upload url is absolute the way Rails' is.
  def self.set_current(request)
    return nil if request.nil?
    ActiveStorage::Current.url_options = {
      protocol: request.base_url.start_with?("https://") ? "https" : "http",
      host: request.host,
    }
    nil
  end

  # The shape the app's own controllers have: `process_action` runs
  # the one action, the dispatcher reads status/body/headers off the
  # base class. `params` carries the route's segments and the query.
  module Blobs
    class RedirectController < ActionController::Base
      def process_action(action_name)
        ActiveStorage.set_current(request)
        show
        nil
      end

      def show
        blob = ActiveStorage::Blob.find_signed(Params.str(@params, "signed_id", ""))
        if blob.nil?
          head(:not_found)
        else
          disposition = Params.str(@params, "disposition", "inline")
          redirect_to(ActiveStorage::DiskKey.disk_url(blob, disposition))
        end
        nil
      end
    end
  end

  # Rails' representations controller: the variation segment is the
  # variation's own encoding (`Variation#encode`, where Rails puts a
  # signed transformation key), decoded and PROCESSED here — the
  # variant record is found or made on this request, as in Rails —
  # then redirected to the variant blob's disk URL. A segment that
  # decodes to nothing is the identity variant: the original.
  module Representations
    class RedirectController < ActionController::Base
      def process_action(action_name)
        ActiveStorage.set_current(request)
        show
        nil
      end

      def show
        blob = ActiveStorage::Blob.find_signed(Params.str(@params, "signed_blob_id", ""))
        if blob.nil?
          head(:not_found)
        else
          variation = ActiveStorage::Variation.decode(Params.str(@params, "variation_key", ""))
          # Rails' `blob.representation(variation)`: a previewable blob
          # (a video) answers its poster's variant, drawn on demand;
          # any other its own.
          image = if blob.previewable?
            ActiveStorage::Preview.new(blob, variation).variant.image_blob
          else
            ActiveStorage::VariantWithRecord.new(blob, variation).image_blob
          end
          redirect_to(ActiveStorage::DiskKey.disk_url(image.nil? ? blob : image, "inline"))
        end
        nil
      end
    end
  end

  # Rails' `DiskController#show`: the bytes, under the content type and
  # disposition the signed key carries. `Content-Disposition` is what
  # makes a "Download" link download; the app's initializer asks for an
  # hour of public caching on this route and gets it.
  # Rails' `DirectUploadsController#create`: allocate the blob row from
  # what the browser declared and answer the blob's attributes plus
  # where to PUT the bytes. Rails mounts it on every app whether or
  # not the app's own forms use direct uploads (campfire's do not: its
  # composer uploads through `MessagesController`), which is why
  # campfire guards it — `config/initializers/active_storage_
  # authentication.rb` includes a session check into this class and
  # adds the `before_action`. That guard reaches here through
  # `initializer_filters`, the seam `lower::module_mixins` writes an
  # initializer's `before_action` into; with none registered it is a
  # no-op and the endpoint is as open as Rails' own.
  #
  # The body is JSON on the wire (Active Storage's JS POSTs
  # `{"blob":{…}}` with `Content-Type: application/json`), which the
  # production dispatcher does not parse into params, so it is read
  # off the raw body here when `blob` did not arrive as a param.
  class DirectUploadsController < ActionController::Base
    def process_action(action_name)
      ActiveStorage.set_current(request)
      initializer_filters(action_name)
      return nil if performed?
      create
      nil
    end

    # The seam an initializer's `before_action` is written into — see
    # `project::apply_module_mixins`. Redefined by the generated reopen
    # at the end of boot.rb when the app registers one.
    def initializer_filters(action_name)
      nil
    end

    def create
      blob = Params.sub(@params, "blob")
      if blob.length == 0 && request.body.length > 0
        parsed = JSON.parse(request.body.read)
        blob = parsed.is_a?(Hash) ? Params.sub(parsed, "blob") : {}
      end
      filename = blob.fetch("filename", "").to_s
      byte_size = blob.fetch("byte_size", "").to_s.to_i
      checksum = blob.fetch("checksum", "").to_s
      content_type = blob.fetch("content_type", "application/octet-stream").to_s
      if filename == "" || checksum == ""
        head(:unprocessable_entity)
        return nil
      end
      record = ActiveStorage::Blob.create_before_direct_upload!(filename, byte_size, checksum, content_type)
      render(direct_upload_json(record, checksum), content_type: "application/json")
      nil
    end

    # Rails' `direct_upload_json`: `blob.as_json(root: false, methods:
    # :signed_id)` merged with the upload url and headers. Written out
    # rather than through a Hash: the values are of four types and a
    # bag of them is the shape a strict target pays for. `created_at`
    # and `attachable_sgid` are not answered — nothing that consumes
    # this response reads them (Active Storage's own JS reads
    # `signed_id` and `direct_upload`).
    def direct_upload_json(blob, checksum)
      q = ->(v) { ActionController::MessageVerifier.json_string(v) }
      "{\"id\":" + blob.id.to_s +
        ",\"key\":" + q.call(blob.key) +
        ",\"filename\":" + q.call(blob.filename.to_s) +
        ",\"content_type\":" + q.call(blob.content_type) +
        ",\"metadata\":{}" +
        ",\"service_name\":\"local\"" +
        ",\"byte_size\":" + blob.byte_size.to_s +
        ",\"checksum\":" + q.call(checksum) +
        ",\"signed_id\":" + q.call(blob.signed_id) +
        ",\"direct_upload\":{\"url\":" + q.call(ActiveStorage::DiskKey.upload_url(blob, checksum)) +
        ",\"headers\":{\"Content-Type\":" + q.call(blob.content_type) + "}}}"
    end
  end

  class DiskController < ActionController::Base
    def process_action(action_name)
      ActiveStorage.set_current(request)
      initializer_filters(action_name)
      return nil if performed?
      if action_name == :update
        update
      else
        show
      end
      nil
    end

    # The seam an initializer's `before_action` is written into — see
    # `DirectUploadsController#initializer_filters`.
    def initializer_filters(action_name)
      nil
    end

    # Rails' `DiskController#update`: the bytes of a direct upload,
    # held to the token the metadata POST signed — the key they go
    # under, and the content type, length and MD5 the browser
    # declared. A token that does not verify is a 404 (Rails' answer:
    # there is no such upload), a body that does not match what was
    # declared is a 422, and a checksum mismatch after the write is
    # Rails' `IntegrityError`: the file is removed and the answer is
    # 422 too. `Content-Type` is compared as a media type — Rails'
    # `content_mime_type` — so a charset parameter does not fail it.
    def update
      decoded = ActiveStorage::DiskKey.decode_upload(Params.str(@params, "encoded_token", ""))
      key = decoded[0]
      if key == ""
        head(:not_found)
        return nil
      end
      body = request.body.read
      declared_type = request.env.fetch("CONTENT_TYPE", "").to_s.split(";")[0].to_s.strip
      if declared_type != decoded[1] || body.bytesize.to_s != decoded[2]
        head(:unprocessable_entity)
        return nil
      end
      service = ActiveStorage::Blob.service
      service.upload(key, body)
      if Digest::MD5.base64digest(body) != decoded[3]
        service.delete(key)
        head(:unprocessable_entity)
        return nil
      end
      head(:no_content)
      nil
    end

    def show
      decoded = ActiveStorage::DiskKey.decode(Params.str(@params, "encoded_key", ""))
      key = decoded[0]
      disposition = decoded[1]
      blob = key == "" ? nil : ActiveStorage::Blob.find_by_key(key)
      service = ActiveStorage::Blob.service
      if blob.nil? || !service.exist?(key)
        head(:not_found)
        return nil
      end
      @headers["Content-Disposition"] = disposition + "; filename=\"" +
        ActiveStorage.url_filename(blob.filename.to_s) + "\""
      @headers["Cache-Control"] = "max-age=3600, public"
      send_data(service.download(key), type: blob.content_type, disposition: disposition)
      nil
    end
  end
end

# `polymorphic_url(record)` for the two Active Storage values — what
# `url_for(attachment)` / `url_for(variant)` resolve to in Rails, and
# what campfire's `broadcast_image_path` calls for anything that is
# not a String. The shared definition raises (a record's route is
# resolved at transpile time); this reopen narrows the two classes
# whose route is a runtime fact and re-raises for the rest.
module ActionView
  module ViewHelpers
    def self.polymorphic_url(record, only_path: false)
      if record.is_a?(ActiveStorage::Attached)
        record.url
      elsif record.is_a?(ActiveStorage::VariantWithRecord)
        record.url
      else
        raise NotImplementedError,
              "ActionView::ViewHelpers.polymorphic_url: a record's route is " \
              "resolved at transpile time — no runtime record-to-route " \
              "mapping is modeled beyond Active Storage's two values"
      end
    end
  end
end
