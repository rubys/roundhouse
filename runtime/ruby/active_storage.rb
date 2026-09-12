# Active Storage — what a `has_one_attached` reader hands back, the
# blob it points at, and the identity "variant" of that blob.
#
# Sits beside `ActionText::Content` and for the same reason: the
# proxy has no table, so it is framework Ruby rather than a
# synthesized model. The ROWS (`active_storage_attachments`,
# `active_storage_blobs`) are read and written here with raw SQL
# through the adapter, the way `Relation` itself composes.
#
# # Three halves, and where each lives
#
# Active Storage is three things: attachment ROWS (which record, which
# blob, under which name), BLOBS (bytes in a service), and VARIANTS
# (derivatives produced by an image processor).
#
# * ROWS are fully modeled here, in the shared runtime: `attached?`,
#   `filename`, `content_type`, `attach`, `purge`, and the preload the
#   batch loader drives.
# * BLOB BYTES go through `ActiveStorage::Service`, whose methods here
#   RAISE: the shared runtime does no file I/O anywhere (grep it), so
#   a storage service is a per-target seam. The ruby family reopens the
#   class with a disk service (`runtime/spinel/active_storage_disk.rb`)
#   under Rails' own `storage/files/<xx>/<yy>/<key>` layout. `Blob
#   .create_and_upload!` and `Attached#attach` write the ROW here and
#   hand the bytes to the service, so a target that has one gets the
#   whole feature and a target that has none fails at the one named
#   seam.
# * VARIANTS are IDENTITY. `variant`, `representation` and `.processed`
#   answer an `ActiveStorage::VariantWithRecord` whose `key` is the
#   original blob's key — no processor exists here, so the "thumb" a
#   view asks for is served at the upload's own size. `variable?` is
#   Rails' content-type question (can a variant be MADE from this),
#   answered from `variable_content_types` so an image goes down the
#   image path and a PDF down the link path exactly as in Rails. What
#   differs is the pixels, and only the pixels: a page that renders an
#   attachment renders the same tags around the same URLs.
#   `preview` (a poster frame for a video, a page for a PDF) is NOT
#   identity — there is no still to serve — so it raises, and
#   `previewable?` is false so no view reaches it.
#
# # The reader never returns nil
#
# Rails' `record.logo` is an `Attached::One` proxy whether or not
# anything is attached, which is why app code writes `logo.attached?`
# and `logo.variable?` without a nil guard. The reader keeps that
# contract by always constructing one of these.
module ActiveStorage
  # Rails' default `config.active_storage.variable_content_types`: the
  # image types a processor could make a variant from. A content-type
  # question, not a bytes question, which is why it can be answered
  # here. An app that trims the list in an initializer (campfire drops
  # bmp/ico/psd, whose loaders it does not trust) has the trim lifted
  # onto its `Rails::Application` reopen at ingest, and `variable?`
  # honours it: a bmp avatar falls back to initials here as it does
  # there.
  def self.default_variable_content_types
    ["image/png", "image/gif", "image/jpeg", "image/tiff", "image/bmp",
     "image/vnd.adobe.photoshop", "image/vnd.microsoft.icon", "image/webp",
     "image/avif", "image/heic", "image/heif"]
  end

  def self.variable_content_types
    excluded = Rails.application.active_storage_excluded_content_types
    out = []
    default_variable_content_types.each do |t|
      out << t unless excluded.include?(t)
    end
    out
  end

  def self.variable_content_type?(content_type)
    variable_content_types.include?(content_type)
  end

  # Where the blob's file lives, keyed by the blob's `key` column.
  #
  # Every method RAISES: no storage service is modeled in the shared
  # runtime, and a plausible answer (an empty string for `download`, a
  # path nothing wrote to) would be a page that renders a broken image
  # — the failure that looks like success. The ruby family reopens this
  # class with a disk implementation; the methods stay the contract.
  #
  # A class rather than a module because `Blob.service` is a SINGLETON
  # in Rails and app code chains off it (`ActiveStorage::Blob.service
  # .path_for(key)`); keeping the shape means the call site needs no
  # rewrite when a real service is present.
  class Service
    def path_for(key)
      raise NotImplementedError,
            "ActiveStorage::Service#path_for: no storage service on this " \
            "target — the shared runtime does no file I/O"
    end

    def upload(key, data)
      raise NotImplementedError,
            "ActiveStorage::Service#upload: no storage service on this " \
            "target — the shared runtime does no file I/O"
    end

    def download(key)
      raise NotImplementedError,
            "ActiveStorage::Service#download: no storage service on this " \
            "target — the shared runtime does no file I/O"
    end

    def delete(key)
      raise NotImplementedError,
            "ActiveStorage::Service#delete: no storage service on this " \
            "target — the shared runtime does no file I/O"
    end

    def exist?(key)
      raise NotImplementedError,
            "ActiveStorage::Service#exist?: no storage service on this " \
            "target — the shared runtime does no file I/O"
    end
  end

  # The `metadata` column, as the two numbers the corpus reads from it
  # (`attachment.metadata[:width]`). Rails' analyzer fills the column
  # with a JSON object (`{"identified":true,"width":…,"height":…,
  # "analyzed":true}`); a blob nothing identified answers nil for both,
  # which is what an app's `width.nil? || height.nil?` guard expects.
  #
  # A class with a Symbol-keyed `[]` rather than a `Hash[Symbol,
  # Integer?]`, so the read is typed on every target rather than a
  # nullable lookup in a bag.
  class BlobMetadata
    def initialize(width, height)
      @width = width
      @height = height
    end

    def [](key)
      if key == :width
        @width == 0 ? nil : @width
      elsif key == :height
        @height == 0 ? nil : @height
      else
        nil
      end
    end

    def width
      @width
    end

    def height
      @height
    end

    # The JSON Rails writes, so a database this tree and a Rails process
    # share reads the same numbers either way.
    def to_json
      if @width == 0 || @height == 0
        "{\"identified\":true,\"analyzed\":true}"
      else
        "{\"identified\":true,\"width\":" + @width.to_s +
          ",\"height\":" + @height.to_s + ",\"analyzed\":true}"
      end
    end

    # The number after `"<name>":` in the column's JSON, or 0. A
    # hand-rolled scan rather than a JSON parse: the column is written
    # by `to_json` above (or by Rails, in the same shape), the two keys
    # are integers, and the shared runtime has no typed JSON reader
    # to hand a Hash back from.
    def self.int_field(json, name)
      needle = "\"" + name + "\":"
      at = json.index(needle)
      return 0 if at.nil?
      i = at + needle.length
      n = 0
      while i < json.length
        c = json[i].to_s
        break if c < "0" || c > "9"
        n = n * 10 + c.to_i
        i += 1
      end
      n
    end

    def self.parse(json)
      BlobMetadata.new(int_field(json, "width"), int_field(json, "height"))
    end
  end

  # One `active_storage_blobs` row: what the file IS, apart from what it
  # is attached to. Constructed from a row (`from_row`), never queried
  # per field.
  class Blob
    def initialize(id, key, filename, content_type, byte_size, metadata)
      @id = id
      @key = key
      @filename = filename
      @content_type = content_type
      @byte_size = byte_size
      @metadata = metadata
    end

    def id
      @id
    end

    def key
      @key
    end

    def filename
      @filename
    end

    def content_type
      @content_type
    end

    def byte_size
      @byte_size
    end

    def metadata
      @metadata
    end

    # The columns every blob read selects, so the proxy's own join, the
    # batch preloader and `find` cannot disagree about the row shape.
    def self.columns(alias_prefix)
      alias_prefix + ".id AS blob_id, " + alias_prefix + ".key AS blob_key, " +
        alias_prefix + ".filename AS filename, " + alias_prefix +
        ".content_type AS content_type, " + alias_prefix +
        ".byte_size AS byte_size, " + alias_prefix + ".metadata AS metadata"
    end

    def self.from_row(row)
      Blob.new(
        row["blob_id"].to_i,
        row["blob_key"].to_s,
        row["filename"].to_s,
        row["content_type"].to_s,
        row["byte_size"].to_i,
        BlobMetadata.parse(row["metadata"].to_s)
      )
    end

    def self.find(id)
      rows = ActiveRecord.adapter.select_rows(
        "SELECT " + columns("b") + " FROM active_storage_blobs b WHERE b.id = " +
        ActiveRecord.adapter.escape_value(id) + " LIMIT 1"
      )
      rows.length == 0 ? nil : from_row(rows[0])
    end

    def self.find_by_key(key)
      rows = ActiveRecord.adapter.select_rows(
        "SELECT " + columns("b") + " FROM active_storage_blobs b WHERE b.key = " +
        ActiveRecord.adapter.escape_value(key) + " LIMIT 1"
      )
      rows.length == 0 ? nil : from_row(rows[0])
    end

    # Rails' `ActiveStorage.verifier` signs the blob id with purpose
    # `blob_id`; the same envelope `record.signed_id` uses, under
    # Active Storage's own salt so a blob token and a record token can
    # never stand in for each other.
    def self.find_signed(signed_id)
      json = ActionController::MessageVerifier.verified_data_json(
        Rails.application.secret_key_base, "ActiveStorage", signed_id, "blob_id", false
      )
      return nil if json == ""
      find(json.to_i)
    end

    def signed_id
      ActionController::MessageVerifier.data_envelope(
        Rails.application.secret_key_base, "ActiveStorage", @id.to_s, "blob_id", "", false
      )
    end

    def self.service
      Service.new
    end

    # A value for the `key` column, which is NOT NULL and uniquely
    # indexed, and which names the file on disk.
    #
    # Derived rather than random: randomness is a per-target seam here
    # (the spinel tree reaches /dev/urandom through FFI, the CRuby tree
    # through SecureRandom), and the shared runtime cannot name either.
    # The filename plus the clock plus the byte count is unique in every
    # way that matters, and a collision would need the same name and
    # size in the same millisecond.
    def self.generate_key(filename, byte_size)
      MessageDigest.hmac_sha256_hex(
        "active_storage/blob/" + byte_size.to_s,
        filename + "/" + ActiveSupport.db_now
      )
    end

    # Rails: the blob row AND the bytes. The row goes through the
    # adapter here; the bytes go to the service, which is the one seam
    # a target without storage fails at. `byte_size` is REAL — `data`
    # is the file's contents — so the column means what it says.
    #
    # `data` is bytes, NOT an io: `lower::attached` grounds Rails'
    # `io:` at the call site (`f.read`), because the shared runtime's
    # RBS has no `File` or `IO` type to name.
    #
    # Analyzed on the way in rather than by a later job, so the
    # `metadata` column carries the image's dimensions from the moment
    # the row exists — campfire's `attachment&.analyze` in its create
    # path then has nothing left to do. The analyzer itself is a
    # per-target seam (it reads bytes; see `ImageAnalyzer`).
    def self.create_and_upload!(data, filename, content_type)
      key = generate_key(filename, data.length)
      dims = ImageAnalyzer.dimensions(data, content_type)
      metadata = BlobMetadata.new(dims[0], dims[1])
      service.upload(key, data)
      id = ActiveRecord.adapter.insert("active_storage_blobs", {
        "key" => key,
        "filename" => filename,
        "content_type" => content_type,
        "metadata" => metadata.to_json,
        "service_name" => "local",
        "byte_size" => data.length,
        "checksum" => "",
        "created_at" => ActiveSupport.db_now,
      })
      Blob.new(id, key, filename, content_type, data.length, metadata)
    end

    # Rails' attachable coercion: what `record.avatar = x` and
    # `create!(avatar: x)` accept — an uploaded file, a blob, a signed
    # blob id. Narrowing an untyped value by class is the ruby family's
    # to do (`runtime/spinel/active_storage_disk.rb` reopens this); the
    # shared runtime declines rather than guessing at a type it cannot
    # test for.
    def self.from_attachable(value)
      raise NotImplementedError,
            "ActiveStorage::Blob.from_attachable: attachable coercion is " \
            "a per-target seam — no multipart body reaches this target"
    end

    def download
      Blob.service.download(@key)
    end

    def purge
      Blob.service.delete(@key)
      ActiveRecord.adapter.delete("active_storage_blobs", @id)
      nil
    end

    def video?
      @content_type.start_with?("video/")
    end

    def image?
      @content_type.start_with?("image/")
    end

    def audio?
      @content_type.start_with?("audio/")
    end

    def variable?
      ActiveStorage.variable_content_type?(@content_type)
    end

    # The blob's own route: `rails_blob_path(blob)`. A `disposition` of
    # "attachment" rides as the query Rails puts it in.
    def url(disposition)
      base = "/rails/active_storage/blobs/redirect/" + signed_id + "/" +
             ActiveStorage.url_filename(@filename)
      disposition == "" ? base : base + "?disposition=" + disposition
    end
  end

  # The filename segment of a blob URL: Rails writes the filename as
  # the last path segment (`*filename`, cosmetic — the key is in the
  # signed id). URI-component encoded so a name with a space or a
  # slash cannot break the path.
  def self.url_filename(filename)
    ActionView::ViewHelpers.url_encode_component(filename)
  end

  # The dimensions of an image, or `[0, 0]` when nothing here can
  # read them. A bytes question: the shared runtime's String is code
  # points, so this is a per-target seam (the ruby family reopens it
  # over `getbyte`). `[0, 0]` is Rails' own state for a blob no
  # analyzer identified — `metadata[:width]` is nil and the view's
  # guard takes its no-dimensions branch.
  class ImageAnalyzer
    def self.dimensions(data, content_type)
      [0, 0]
    end
  end

  # Rails' `ActiveStorage::VariantWithRecord`, the value `variant(:x)`
  # and `representation(:x)` answer. IDENTITY: `processed` is self and
  # `key` is the original blob's key, so `Blob.service.path_for
  # (variant.key)` serves the upload as it was. `image` is the
  # attachment the variant was asked of — in Rails it is the variant
  # record's own attachment, and here the two are the same bytes.
  class VariantWithRecord
    def initialize(attached, name)
      @attached = attached
      @name = name
    end

    def processed
      self
    end

    def image
      @attached
    end

    def blob
      @attached.blob
    end

    def key
      @attached.key
    end

    # `url_for(variant)` / `polymorphic_url(variant)`: Rails'
    # representation route, with the variant's name where Rails puts
    # the encoded transformation. The disk controller serves the
    # original for any variation, which is what identity means.
    def url
      "/rails/active_storage/representations/redirect/" + @attached.signed_id +
        "/" + @name + "/" + ActiveStorage.url_filename(@attached.filename.to_s)
    end
  end

  class Attached
    # Constructed with the three columns Rails scopes an attachment on,
    # NOT with a pre-computed boolean: a boolean argument would have
    # meant folding a `where(...).exists?` chain into the reader — which
    # the query specializer inlines into a multi-statement SQL block that
    # cannot sit in an argument position. Raw SQL through the adapter,
    # the way `Relation` itself composes, keeps the reader a plain
    # constructor call.
    #
    # The row is read ONCE and remembered, as Rails' `Attached::One`
    # remembers its attachment until `reload`. An earlier version of
    # this class argued the other way — a record can be attached to
    # between two reads, so ask every time — and that argument was
    # overturned deliberately: it cost two round trips per message on
    # campfire's room page (`content_type` asks `attached?` twice), and
    # it is not what Rails does. `attach` and `purge` forget the row, so
    # a write through this proxy is seen by its next read.
    def initialize(record_type, record_id, name)
      @record_type = record_type
      @record_id = record_id
      @name = name
      @row_loaded = false
      @attachment_id = 0
      @blob = nil
    end

    # The attachment row and its blob, in one join. `attachment_id` 0
    # means nothing is attached.
    def load_row
      return nil if @row_loaded
      @row_loaded = true
      sql = "SELECT a.id AS attachment_id, " + Blob.columns("b") +
            " FROM active_storage_attachments a " +
            "JOIN active_storage_blobs b ON b.id = a.blob_id WHERE a.record_type = " +
            ActiveRecord.adapter.escape_value(@record_type) +
            " AND a.record_id = " + ActiveRecord.adapter.escape_value(@record_id) +
            " AND a.name = " + ActiveRecord.adapter.escape_value(@name) + " LIMIT 1"
      rows = ActiveRecord.adapter.select_rows(sql)
      if rows.length > 0
        @attachment_id = rows[0]["attachment_id"].to_i
        @blob = Blob.from_row(rows[0])
      end
      nil
    end

    # The batch loader's entry (`Model._preload_batch_<attr>_attachment`,
    # driven by `with_attached_<attr>`): the row for this record, found
    # in one query for the whole page, or `0` and no blob when the
    # page's query found none for it.
    def _preload_row(attachment_id, blob)
      @row_loaded = true
      @attachment_id = attachment_id
      @blob = blob
      nil
    end

    def attached?
      load_row
      @attachment_id != 0
    end

    # The blob, or nil when nothing is attached — Rails' `Attached::One`
    # delegates to its attachment with `allow_nil: true`.
    def blob
      load_row
      @blob
    end

    def filename
      b = blob
      b.nil? ? nil : b.filename
    end

    def content_type
      b = blob
      b.nil? ? nil : b.content_type
    end

    def key
      b = blob
      b.nil? ? "" : b.key
    end

    def signed_id
      b = blob
      b.nil? ? "" : b.signed_id
    end

    def byte_size
      b = blob
      b.nil? ? 0 : b.byte_size
    end

    # `attachment.metadata[:width]` — an unattached proxy answers empty
    # metadata, so the read is nil rather than a NoMethodError.
    def metadata
      b = blob
      b.nil? ? BlobMetadata.new(0, 0) : b.metadata
    end

    # Rails' media predicates, which are content-type questions and so
    # metadata questions. All answer FALSE when nothing is attached,
    # which is what `allow_nil` delegation gives Rails and what lets
    # campfire's `case when attachment.video? … when attachment
    # .representable?` fall through to neither branch.
    def video?
      content_type.to_s.start_with?("video/")
    end

    def image?
      content_type.to_s.start_with?("image/")
    end

    def audio?
      content_type.to_s.start_with?("audio/")
    end

    # Rails: "can a variant be produced from this blob's content type?"
    # Answered from the content type, as Rails does — the processor
    # here is identity, so the answer decides which PATH a view takes
    # (inline image vs. download link), not whether pixels change.
    def variable?
      b = blob
      b.nil? ? false : b.variable?
    end

    # Rails: a previewer (ffmpeg for video, poppler/mutool for PDF)
    # can produce a still. None exists here, so no. Distinct from
    # `variable?` on purpose: a video is not served as its own poster.
    def previewable?
      false
    end

    # Rails: `variable? || previewable?`.
    def representable?
      variable?
    end

    # Rails' `analyze` reads the blob's bytes to fill its metadata.
    # The bytes were analyzed when they were uploaded (`Blob
    # .create_and_upload!`), so there is nothing left to do — nil is
    # the `allow_nil` delegation's own answer for an unattached one.
    def analyze
      nil
    end

    # The identity variant — see the class header and
    # `VariantWithRecord`. `transformations` is the named variant
    # (`:thumb`) or an inline hash; either way the answer is the
    # original, so the name is kept for the URL and the hash is not
    # read.
    def variant(transformations)
      VariantWithRecord.new(self, variant_name(transformations))
    end

    def representation(transformations)
      VariantWithRecord.new(self, variant_name(transformations))
    end

    def variant_name(transformations)
      if transformations.is_a?(Symbol)
        transformations.to_s
      else
        "variant"
      end
    end

    # A poster frame or a page image: nothing here can produce one, and
    # serving the original in its place would put a video's bytes in an
    # `<img>`. Unreachable through the app's own guards
    # (`previewable?` is false), so a call that lands here is a view
    # that skipped them.
    def preview(transformations)
      raise NotImplementedError,
            "ActiveStorage::Attached#preview: no previewer is modeled — " \
            "`previewable?` is false, so this call skipped Rails' own guard"
    end

    # `url_for(attachment)` / `polymorphic_url(attachment)`: the blob's
    # own route, inline.
    def url
      b = blob
      b.nil? ? "" : b.url("")
    end

    # Rails' `attach`, the row half: point this record at `blob` under
    # `@name`. `has_one_attached` REPLACES, so the prior attachment goes
    # first. Rails detaches the old blob and leaves it for a purge job;
    # there is no job here and an orphaned blob row would make
    # `attached?` answer for a file no longer attached, so the row and
    # its bytes go with it.
    def attach_blob(blob)
      purge
      ActiveRecord.adapter.insert("active_storage_attachments", {
        "name" => @name,
        "record_type" => @record_type,
        "record_id" => @record_id,
        "blob_id" => blob.id,
        "created_at" => ActiveSupport.db_now,
      })
      @row_loaded = false
      nil
    end

    # `logo.attach io:, filename:, content_type:` — the blob is created
    # and uploaded, then attached. DATA, NOT AN IO: `lower::attached
    # ::apply_attach_lowering` rewrites Rails' `attach(io: f, …)` to
    # `attach(f.read, …)` at the call site (see `Blob
    # .create_and_upload!` for why).
    def attach(data, filename, content_type)
      attach_blob(Blob.create_and_upload!(data, filename, content_type))
    end

    # Detach and delete: the attachment row, the blob row, and the
    # bytes — see `attach_blob` on why the blob goes too.
    def purge
      sql = "SELECT a.id AS attachment_id, " + Blob.columns("b") +
            " FROM active_storage_attachments a " +
            "JOIN active_storage_blobs b ON b.id = a.blob_id WHERE a.record_type = " +
            ActiveRecord.adapter.escape_value(@record_type) +
            " AND a.record_id = " + ActiveRecord.adapter.escape_value(@record_id) +
            " AND a.name = " + ActiveRecord.adapter.escape_value(@name)
      ActiveRecord.adapter.select_rows(sql).each do |row|
        ActiveRecord.adapter.delete("active_storage_attachments", row["attachment_id"].to_i)
        Blob.from_row(row).purge
      end
      @row_loaded = false
      @attachment_id = 0
      @blob = nil
      nil
    end

    # `account.logo.destroy` — Rails' `Attached::One` has no `destroy`
    # of its own; the call falls through to the ATTACHMENT record, and
    # destroying that removes the join row while leaving the blob to a
    # `purge_later` job. There is no job here, so this is `purge`
    # under Rails' other name.
    def destroy
      purge
    end
  end
end

# Active Storage's ROUTE helpers, reopened onto the app's generated
# `RouteHelpers` module.
#
# They live here rather than in `lower::routes_to_library` because they
# are not the app's routes: Rails mounts them from the Active Storage
# engine, so `config/routes.rb` never names them and the generator that
# reads it cannot know they exist. The routes themselves are served by
# the ruby family's `ActiveStorage::Routes` (the engine's three
# controllers), which is why the URL shapes here are Rails' own:
#
#   /rails/active_storage/blobs/redirect/:signed_id/*filename
#   /rails/active_storage/representations/redirect/:signed_blob_id/:variation_key/*filename
#   /rails/active_storage/disk/:encoded_key/*filename
#
# `disposition` and `only_path` are spelled out because those are the
# two options an ingested app has written. `_url` is the same string as
# `_path`: the app's own `_url` helpers are lowered host-less too.
module RouteHelpers
  def self.rails_blob_path(attachment, disposition: nil, only_path: nil)
    b = attachment.blob
    b.nil? ? "" : b.url(disposition.to_s)
  end

  def self.rails_blob_url(attachment, disposition: nil, only_path: nil)
    rails_blob_path(attachment, disposition: disposition, only_path: only_path)
  end

  def self.rails_representation_path(variant, only_path: nil)
    variant.url
  end

  def self.rails_representation_url(variant, only_path: nil)
    variant.url
  end
end
