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
    # NOT with a pre-computed boolean: the query has to run at ASK time
    # (a record can be attached to between two reads), and a boolean
    # argument would also have meant folding a `where(...).exists?`
    # chain into the reader — which the query specializer inlines into
    # a multi-statement SQL block that cannot sit in an argument
    # position. Raw SQL through the adapter, the way `Relation` itself
    # composes, keeps the reader a plain constructor call.
    def initialize(record_type, record_id, name)
      @record_type = record_type
      @record_id = record_id
      @name = name
    end

    def attached?
      sql = "SELECT id FROM active_storage_attachments WHERE record_type = " +
            ActiveRecord.adapter.escape_value(@record_type) +
            " AND record_id = " + ActiveRecord.adapter.escape_value(@record_id) +
            " AND name = " + ActiveRecord.adapter.escape_value(@name) + " LIMIT 1"
      ActiveRecord.adapter.select_rows(sql).length > 0
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
      blob_column("filename")
    end

    # One blob column of the attached blob, or nil when nothing is
    # attached. METADATA, not bytes — the values live in columns, so
    # this is a join away and needs no storage service, which is why
    # these can answer where `variant` / `url` / `download` still raise.
    #
    # Column name interpolated rather than escaped: every caller is a
    # method on this class passing a literal, so there is no untrusted
    # value here — and a column name is not a value SQLite would take
    # a bound parameter for anyway.
    def blob_column(column)
      sql = "SELECT b." + column + " FROM active_storage_attachments a " +
            "JOIN active_storage_blobs b ON b.id = a.blob_id WHERE a.record_type = " +
            ActiveRecord.adapter.escape_value(@record_type) +
            " AND a.record_id = " + ActiveRecord.adapter.escape_value(@record_id) +
            " AND a.name = " + ActiveRecord.adapter.escape_value(@name) + " LIMIT 1"
      rows = ActiveRecord.adapter.select_rows(sql)
      rows.length > 0 ? rows[0][column] : nil
    end

    # The blob's content type, or nil when nothing is attached — same
    # column, same join, same `allow_nil` contract as `filename`.
    def content_type
      blob_column("content_type")
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
      nil
    end
  end
end
