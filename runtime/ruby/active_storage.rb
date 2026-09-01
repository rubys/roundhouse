# Active Storage's value type — what a `has_one_attached` reader hands
# back.
#
# Sits beside `ActionText::Content` and for the same reason: it has no
# table, so it is framework Ruby rather than a synthesized model. The
# ROW half (`ActiveStorage::Attachment`) is a model, synthesized by
# `lower::attached` onto `app.models`.
#
# NEVER NIL. Rails' `record.logo` is an `Attached::One` proxy whether
# or not anything is attached, which is why app code writes
# `logo.attached?` and `logo.variable?` without a nil guard. The reader
# keeps that contract by always constructing one of these.
#
# What it will NOT answer: blobs and variants. `variant`, `url`,
# `download`, `attach` and `purge` are the bytes half of Active
# Storage — a storage service, a processor, and a variant-records
# table, none of which exist here. They raise rather than return
# something plausible, because a page that renders a broken image URL
# is a failure that looks like success.
module ActiveStorage
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
      @blob_filename = ""
      @blob_content_type = ""
    end

    # The attachment row and the two blob columns the readers answer
    # from, in one join. `attachment_id` 0 means nothing is attached.
    def load_row
      return nil if @row_loaded
      @row_loaded = true
      sql = "SELECT a.id AS attachment_id, b.filename AS filename, b.content_type AS content_type " +
            "FROM active_storage_attachments a " +
            "JOIN active_storage_blobs b ON b.id = a.blob_id WHERE a.record_type = " +
            ActiveRecord.adapter.escape_value(@record_type) +
            " AND a.record_id = " + ActiveRecord.adapter.escape_value(@record_id) +
            " AND a.name = " + ActiveRecord.adapter.escape_value(@name) + " LIMIT 1"
      rows = ActiveRecord.adapter.select_rows(sql)
      if rows.length > 0
        @attachment_id = rows[0]["attachment_id"].to_i
        @blob_filename = rows[0]["filename"].to_s
        @blob_content_type = rows[0]["content_type"].to_s
      end
      nil
    end

    # The batch loader's entry (`Model._preload_batch_<attr>_attachment`,
    # driven by `with_attached_<attr>`): the row for this record, found
    # in one query for the whole page, or `0` and blanks when the page's
    # query found none for it.
    def _preload_row(attachment_id, filename, content_type)
      @row_loaded = true
      @attachment_id = attachment_id
      @blob_filename = filename
      @blob_content_type = content_type
      nil
    end

    def attached?
      load_row
      @attachment_id != 0
    end

    # Rails: "can a variant be produced from this blob's content type?"
    # Without blobs the answer is no, and it is honest — nothing here
    # can produce a variant. Guards `logo_variant`-shaped methods
    # (`logo.variant(size).processed if logo.variable?`) into their nil
    # branch rather than into a raise.
    def variable?
      false
    end

    def variant(_transformations)
      raise NotImplementedError,
            "ActiveStorage variants are not modeled: no blob store, no processor"
    end

    def url
      raise NotImplementedError, "ActiveStorage blob URLs are not modeled"
    end

    # The blob's filename, or NIL when nothing is attached — Rails'
    # `Attached::One` delegates to its attachment with `allow_nil:
    # true`, which is what lets campfire write
    # `attachment&.filename&.to_s` on every message.
    #
    def filename
      load_row
      @attachment_id == 0 ? nil : @blob_filename
    end

    # The blob's content type, or nil when nothing is attached — same
    # row, same `allow_nil` contract as `filename`. METADATA, not bytes:
    # the values live in columns, so they are a join away and need no
    # storage service, which is why these can answer where `variant` /
    # `url` / `download` still raise.
    def content_type
      load_row
      @attachment_id == 0 ? nil : @blob_content_type
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

    # Rails: "can this be previewed or turned into a variant?" Both
    # halves need the bytes, so the answer here is the same `false`
    # `variable?` gives, for the same reason.
    def representable?
      false
    end

    # Rails' `analyze` reads the blob's bytes to fill its metadata.
    # There are no bytes, and nothing can attach any (`attach` raises),
    # so there is never a blob to analyze — nil is the `allow_nil`
    # delegation's own answer for an unattached one. campfire calls this
    # on every message commit (`attachment&.analyze`), so a raise here
    # would take down the message path for a gap that costs it nothing.
    def analyze
      nil
    end

    # `logo.attach io:, filename:, content_type:` — the METADATA half of
    # Rails' attach, which is the half this class has always answered.
    # Two rows: the blob (what the file IS) and the attachment (what it
    # is attached TO). Everything the readers above join for —
    # `attached?`, `filename`, `content_type`, `image?` — becomes true
    # of the record the moment these land.
    #
    # NO BYTES ARE STORED. The shared runtime does no file I/O anywhere
    # (grep it), because a storage service is a per-target seam rather
    # than transpiled Ruby, so `url` / `download` /
    # `Blob.service.path_for` still raise and this method does not
    # pretend otherwise. `byte_size` is nonetheless REAL — `data` is the
    # file's actual contents — so the column means what it says rather
    # than carrying a zero nobody can trust.
    #
    # DATA, NOT AN IO. `lower::attached::apply_attach_lowering` rewrites
    # Rails' `attach(io: f, filename:, content_type:)` to
    # `attach(f.read, …)` at the call site, because the shared runtime's
    # RBS has no `File` or `IO` type to name and an `untyped` parameter
    # here costs every strict target. Same grounding
    # `signed_id(expires_in:)` and the controller's `expires_in` get.
    #
    # `has_one_attached` REPLACES, so the prior attachment goes first.
    # Rails detaches the old blob and leaves it for a purge job; there
    # is no job here and an orphaned blob row would make `attached?`
    # answer for a file no longer attached, so the row goes with it.
    def attach(data, filename, content_type)
      purge
      @row_loaded = false
      blob_id = ActiveRecord.adapter.insert("active_storage_blobs", {
        "key" => blob_key(filename),
        "filename" => filename,
        "content_type" => content_type,
        "metadata" => "{}",
        "service_name" => "local",
        "byte_size" => data.length,
        "checksum" => "",
        "created_at" => ActiveSupport.db_now,
      })
      ActiveRecord.adapter.insert("active_storage_attachments", {
        "name" => @name,
        "record_type" => @record_type,
        "record_id" => @record_id,
        "blob_id" => blob_id,
        "created_at" => ActiveSupport.db_now,
      })
      nil
    end

    # A value for the blob's `key` column, which is NOT NULL and
    # uniquely indexed.
    #
    # Derived rather than random: randomness is a per-target seam here
    # (the spinel tree reaches /dev/urandom through FFI, the CRuby tree
    # through SecureRandom), and the shared runtime cannot name either.
    # The attachment's own identity plus the clock is unique in every
    # way that matters — two different records cannot collide at all,
    # and one record cannot hold two attachments under the same name,
    # because `attach` purges first.
    #
    # It is not Rails' key and nothing round-trips through it yet: no
    # bytes are stored, so no service ever looks a file up by it.
    def blob_key(filename)
      MessageDigest.hmac_sha256_hex(
        @record_type + "/" + @record_id.to_s + "/" + @name,
        filename + "/" + ActiveSupport.db_now
      )
    end

    # Detach and delete. The rows only — see `attach` on why there are
    # no bytes to delete beside them.
    def purge
      sql = "SELECT a.id AS attachment_id, b.id AS blob_id " +
            "FROM active_storage_attachments a " +
            "JOIN active_storage_blobs b ON b.id = a.blob_id WHERE a.record_type = " +
            ActiveRecord.adapter.escape_value(@record_type) +
            " AND a.record_id = " + ActiveRecord.adapter.escape_value(@record_id) +
            " AND a.name = " + ActiveRecord.adapter.escape_value(@name)
      ActiveRecord.adapter.select_rows(sql).each do |row|
        ActiveRecord.adapter.delete("active_storage_attachments", row["attachment_id"].to_i)
        ActiveRecord.adapter.delete("active_storage_blobs", row["blob_id"].to_i)
      end
      @row_loaded = false
      nil
    end

    # `account.logo.destroy` — Rails' `Attached::One` has no `destroy`
    # of its own; the call falls through to the ATTACHMENT record, and
    # destroying that removes the join row while leaving the blob to a
    # `purge_later` job.
    #
    # There is no job here, and an orphaned blob row would make
    # `attached?` answer for a file no longer attached — the same
    # reasoning `attach`'s replace-first already carries. So this is
    # `purge` under Rails' other name.
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
# reads it cannot know they exist. A view that writes
# `rails_blob_path(message.attachment, disposition: "attachment")` —
# campfire's download link, on every attachment message — was emitted as
# a call to a method NOTHING defined. On CRuby that is a NoMethodError at
# render time; on a strict target it is `unsupported call:
# (CallNode 'rails_blob_path')` and the whole build stops.
#
# They RAISE, in the same voice and for the same reason as
# `Attached#url` above: the bytes half of Active Storage is a storage
# service, a processor and a signed-id scheme, none of which exist here.
# A plausible-looking URL would be a page that renders a broken image —
# the failure that looks like success. What changes is only that the gap
# now has ONE named home that every target compiles, instead of a
# missing method that reads like a compiler bug.
#
# `disposition` and `only_path` are spelled out because those are the
# two options an ingested app has written; a bag would buy nothing here,
# since the body cannot use them either way.
# `ActiveStorage::Blob` — the CLASS-side surface an app reaches directly,
# and the last unnamed corner of the bytes half.
#
# Same voice, same reason as `RouteHelpers.rails_blob_path` below and
# `Attached#url` above: no storage service exists here, so nothing can
# answer where a key's file lives or put one there. campfire writes both
# — `ActiveStorage::Blob.service.path_for(variant.key)` serves the
# custom account logo and a bot's webp avatar,
# `ActiveStorage::Blob.create_and_upload!` stores a webhook's attachment
# reply — and until this existed each was an unresolved call that stopped
# the whole strict build, which reads like a compiler bug rather than
# the gap it is.
#
# `Service` is a class rather than a module because `Blob.service` is a
# SINGLETON in Rails and app code chains off it; keeping the shape means
# the call site needs no rewrite when a real service lands.
module ActiveStorage
  class Service
    def path_for(key)
      raise NotImplementedError,
            "ActiveStorage::Blob.service.path_for: no storage service is " \
            "modeled — the shared runtime does no file I/O; see " \
            "ActiveStorage::Attached#url"
    end
  end

  class Blob
    def self.service
      Service.new
    end

    # Rails builds the blob row AND writes the bytes. The row half is
    # what `Attached#attach` already does for an attachment; this one
    # is reached with no record to attach to, so there is nothing to
    # write the row FOR until the caller attaches it — and the bytes
    # have nowhere to go regardless.
    def self.create_and_upload!(io, filename, content_type)
      raise NotImplementedError,
            "ActiveStorage::Blob.create_and_upload!: no storage service is " \
            "modeled — the shared runtime does no file I/O; see " \
            "ActiveStorage::Attached#url"
    end
  end
end

module RouteHelpers
  def self.rails_blob_path(attachment, disposition: nil, only_path: nil)
    raise NotImplementedError,
          "RouteHelpers.rails_blob_path: Active Storage blob URLs are not " \
          "modeled (no storage service, no signed ids) — see " \
          "ActiveStorage::Attached#url"
  end

  def self.rails_blob_url(attachment, disposition: nil, only_path: nil)
    raise NotImplementedError,
          "RouteHelpers.rails_blob_url: Active Storage blob URLs are not " \
          "modeled (no storage service, no signed ids) — see " \
          "ActiveStorage::Attached#url"
  end
end
