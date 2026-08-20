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

    def attach(_attachable)
      raise NotImplementedError, "ActiveStorage attach is not modeled"
    end

    def purge
      raise NotImplementedError, "ActiveStorage purge is not modeled"
    end
  end
end
