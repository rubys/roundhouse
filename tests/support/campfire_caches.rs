//! One contract per construct campfire's SQLite-observer caches
//! (rubys/roundhouse#698) brought into the subset, shared by the
//! interpreted (CRuby overlay, `tests/emit_and_run/campfire_caches.rs`)
//! and native (spinel, `tests/spinel_toolchain/campfire_caches.rs`)
//! lanes. Each removed a strict-emit error or an ingest gap; the two
//! lanes run the same source and print the same lines.

use super::emit_and_run::{real_blog, Overlay};

/// A probe the overlay writes, the script that drives it, and the
/// lines the script prints (CRuby's output for the same code).
pub struct Contract {
    pub path: &'static str,
    pub source: &'static str,
    pub script: &'static str,
    pub expected: &'static str,
}

impl Contract {
    pub fn overlay(&self) -> Overlay {
        real_blog().write(self.path, self.source)
    }
}

/// `_, (_, removed_size) = @entries.shift` — `ResponseCache#write`'s
/// eviction. Each group destructures its own element, a scalar group
/// value pads with nil, and the assignment evaluates to its RHS.
pub const NESTED_MULTI_WRITE: Contract = Contract {
    path: "app/models/nested_write_probe.rb",
    source: r#"class NestedWriteProbe
  def initialize
    @entries = { "first" => ["a", 3], "second" => ["b", 5] }
    @bytes = 8
  end

  def evict
    _, (_, removed_size) = @entries.shift
    @bytes -= removed_size
    @bytes
  end

  def remaining
    @entries.length
  end

  def deep
    a, (b, (c, d)) = 1, [2, [3, 4]]
    [a, b, c, d]
  end

  def padded
    a, (b, c) = 1, 2
    [a, b, c.nil?]
  end
end
"#,
    script: r#"probe = NestedWriteProbe.new
puts probe.evict
puts probe.evict
puts probe.remaining
puts probe.deep.inspect
puts probe.padded.inspect
"#,
    expected: "5\n0\n0\n[1, 2, 3, 4]\n[1, 2, true]\n",
};

/// `ContentKey = Data.define(:digest) do def cache_key … end` —
/// `FragmentCache`'s content key. The block's methods (instance and
/// class side) are the Data class's, beside its member readers.
pub const DATA_BLOCK_METHODS: Contract = Contract {
    path: "app/models/data_block_probe.rb",
    source: r#"class DataBlockProbe
  ContentKey = Data.define(:digest) do
    def cache_key
      "key-" + private_digest
    end

    private

    def private_digest
      digest
    end

    # The enclosing visibility marker does not make an explicit singleton
    # method private; Ruby keeps `build` public.
    def self.build(value)
      new(digest: value)
    end

    public

    def encoded_digest
      DataKeySupportController.encode(digest)
    end
  end

  EmptyKey = Data.define(:value) do
  end

  def self.key(value)
    ContentKey.new(digest: value)
  end
end

class OtherDataBlockProbe
  ContentKey = Data.define(:digest) do
    def cache_key
      "other-" + digest
    end
  end
end
"#,
    script: r#"key = DataBlockProbe.key("abc")
puts key.cache_key
puts key.digest
puts key.encoded_digest
puts DataBlockProbe::ContentKey.build("def").cache_key
puts OtherDataBlockProbe::ContentKey.new(digest: "ghi").cache_key
puts key == DataBlockProbe::ContentKey.new(digest: "abc")
puts key.is_a?(DataBlockProbe::ContentKey)
puts DataBlockProbe::EmptyKey.new(value: "empty").value
"#,
    expected: "key-abc\nabc\nencoded-abc\nkey-def\nother-ghi\ntrue\ntrue\nempty\n",
};

/// `CachedResponses`' request surface: rack's encoding negotiation
/// (`Rack::Utils`), the gzip body and its weak ETag (`Zlib.gzip`,
/// `String#byteslice`), the session snapshot (`Hash#deep_dup`) and the
/// cache key (`ActiveSupport::JSON.encode`, with `decode` beside it as
/// `RecordCache` reads it back).
pub const RESPONSE_HELPERS: Contract = Contract {
    path: "app/models/response_helpers_probe.rb",
    source: r##"require "zlib"

class ResponseHelpersProbe
  def self.encoding(header)
    Rack::Utils.select_best_encoding(%w[ gzip identity ], Rack::Utils.q_values(header))
  end

  def self.etag
    body = "<p>cached</p>"
    %(W/"#{Digest::SHA256.hexdigest(body).byteslice(0, 32)}")
  end

  def self.past_the_end
    "abc".byteslice(5, 1).nil?
  end

  def self.gzip_round_trip
    body = "<p>cached</p>" * 20
    zipped = Zlib.gzip(body)
    [zipped.byteslice(0, 2).bytes, zipped.bytesize < body.bytesize, Zlib.gunzip(zipped) == body]
  end

  # Equal, but no level shares an object with the original — what the
  # copy is for. (Read, not mutated: mutating through the untyped copy
  # retypes unrelated String-keyed hashes on spinel; see the PR.)
  def self.snapshot
    original = { "user" => { "id" => "1" }, "flash" => [ "a" ] }
    copy = original.deep_dup
    [ copy == original, copy.equal?(original), copy["user"].equal?(original["user"]),
      copy["flash"].equal?(original["flash"]), copy["user"]["id"].equal?(original["user"]["id"]) ]
  end

  def self.key
    ActiveSupport::JSON.encode([ "rooms", "/rooms/1?a=<b>&c", nil, 7, 1.5, true, :html, { "token" => "t", "n" => [ 1, nil ] } ])
  end

  def self.decoded
    ActiveSupport::JSON.decode(key)
  end
end
"##,
    script: r#"puts ResponseHelpersProbe.encoding("gzip, deflate").inspect
puts ResponseHelpersProbe.encoding("br, gzip;q=0.5, *;q=0.1").inspect
puts ResponseHelpersProbe.encoding("gzip;q=0").inspect
puts ResponseHelpersProbe.encoding("*;q=0").inspect
puts ResponseHelpersProbe.encoding(nil).inspect
puts ResponseHelpersProbe.etag
puts ResponseHelpersProbe.past_the_end
puts ResponseHelpersProbe.gzip_round_trip.inspect
puts ResponseHelpersProbe.snapshot.inspect
puts ResponseHelpersProbe.key
puts ResponseHelpersProbe.decoded.inspect
"#,
    expected: concat!(
        "\"gzip\"\n",
        "\"gzip\"\n",
        "\"identity\"\n",
        "nil\n",
        "\"identity\"\n",
        "W/\"dd818157edf943fbb589e25ab389d2c5\"\n",
        "true\n",
        "[[31, 139], true, true]\n",
        "[true, false, false, false, false]\n",
        "[\"rooms\",\"/rooms/1?a=\\u003cb\\u003e\\u0026c\",null,7,1.5,true,\"html\",{\"token\":\"t\",\"n\":[1,null]}]\n",
        "[\"rooms\", \"/rooms/1?a=<b>&c\", nil, 7, 1.5, true, \"html\", {\"token\" => \"t\", \"n\" => [1, nil]}]\n",
    ),
};

/// `ResponseCache`'s observer and `SqliteWalCheckpoint`'s connection:
/// `PRAGMA data_version` holds still on its own connection until another
/// connection commits; a read-only connection refuses writes; a missing
/// file cannot be opened read-only; the block form closes; rows are
/// Arrays of native column values. Beside it the Rails surface the two
/// read — `connection_db_config`, `SQLite3Adapter.resolve_path`,
/// `connection_pool.with_connection(&:transaction_open?)` — and the
/// checkpointer's `flock` lock file.
pub const SQLITE_OBSERVER: Contract = Contract {
    path: "app/models/sqlite_observer_probe.rb",
    source: r##"require "fileutils"

class SqliteObserverProbe
  DIR = "tmp/sqlite_observer_probe"

  def self.observe
    FileUtils.rm_rf(DIR)
    FileUtils.mkdir_p(DIR)
    path = File.join(DIR, "observed.sqlite3")
    SQLite3::Database.new(path) do |db|
      db.execute("PRAGMA journal_mode=WAL")
      db.execute("CREATE TABLE notes (body TEXT)")
    end
    observer = SQLite3::Database.new(path, readonly: true)
    first = observer.get_first_value("PRAGMA data_version")
    still = observer.get_first_value("PRAGMA data_version") == first
    SQLite3::Database.new(path) { |db| db.execute("INSERT INTO notes VALUES ('committed')") }
    moved = observer.get_first_value("PRAGMA data_version") != first
    rows = observer.execute("SELECT body, 7, 2.5, NULL FROM notes")
    refused = begin
      observer.execute("INSERT INTO notes VALUES ('refused')")
      "written"
    rescue SQLite3::Exception
      "refused"
    end
    observer.close
    missing = begin
      SQLite3::Database.new(File.join(DIR, "missing.sqlite3"), readonly: true)
      "opened"
    rescue SQLite3::Exception
      "cannot open"
    end
    checkpoint = nil
    held = SQLite3::Database.new(path) do |db|
      db.busy_handler_timeout = 1_000
      checkpoint = db.execute("PRAGMA wal_checkpoint(PASSIVE)").first
    end
    [ first.is_a?(Integer), still, moved, rows, refused, missing, checkpoint.length, checkpoint[0], held.closed?, observer.closed? ]
  end

  def self.config
    config = ActiveRecord::Base.connection_db_config
    [ config.database, config.adapter ]
  end

  def self.resolved
    [
      ActiveRecord::ConnectionAdapters::SQLite3Adapter.resolve_path("storage/test.sqlite3", root: "/app"),
      ActiveRecord::ConnectionAdapters::SQLite3Adapter.resolve_path("file:/data/x.sqlite3?mode=ro", root: "/app"),
      ActiveRecord::ConnectionAdapters::SQLite3Adapter.resolve_path("file:///data/y.sqlite3", root: "/app"),
      ActiveRecord::ConnectionAdapters::SQLite3Adapter.resolve_path("file:rel.sqlite3?mode=memory", root: "/app")
    ]
  end

  def self.transactions
    outside = ActiveRecord::Base.connection_pool.with_connection(&:transaction_open?)
    inside = ActiveRecord::Base.transaction { ActiveRecord::Base.connection_pool.with_connection(&:transaction_open?) }
    [ outside, inside ]
  end

  def self.locks
    FileUtils.mkdir_p(File.join(DIR, "pids"))
    lock_path = File.join(DIR, "pids", "probe.lock")
    first = File.open(lock_path, File::RDWR | File::CREAT, 0644)
    taken = first.flock(File::LOCK_EX | File::LOCK_NB)
    second = File.open(lock_path, File::RDWR | File::CREAT, 0644)
    busy = second.flock(File::LOCK_EX | File::LOCK_NB)
    first.flock(File::LOCK_UN)
    first.close
    retaken = second.flock(File::LOCK_EX | File::LOCK_NB)
    second.close
    [ taken, busy, retaken ]
  end
end
"##,
    script: concat!(
        "# The native consumer boots libraries without a database; give it the\n",
        "# in-memory one the CRuby overlay's `run_ruby` configures.\n",
        "if ActiveRecord.adapter.nil?\n",
        "  SqliteAdapter.configure(\":memory:\")\n",
        "  ActiveRecord.adapter = SqliteAdapter\n",
        "  Schema.statements.each { |sql| SqliteAdapter.execute_ddl(sql) }\n",
        "end\n",
        r#"puts SqliteObserverProbe.observe.inspect
puts SqliteObserverProbe.config.inspect
puts SqliteObserverProbe.resolved.inspect
puts SqliteObserverProbe.transactions.inspect
puts SqliteObserverProbe.locks.inspect
"#
    ),
    expected: concat!(
        "[true, true, true, [[\"committed\", 7, 2.5, nil]], \"refused\", \"cannot open\", 3, 0, true, true]\n",
        "[\":memory:\", \"sqlite3\"]\n",
        "[\"/app/storage/test.sqlite3\", \"/data/x.sqlite3\", \"/data/y.sqlite3\", \"/app/rel.sqlite3\"]\n",
        "[false, true]\n",
        "[0, false, 0]\n",
    ),
};

/// `RecordCache` and `FragmentCache`: a record's raw attributes, through
/// JSON and back with `name.constantize.instantiate(attributes)`, are a
/// persisted, unchanged copy (timestamps to the microsecond); a String-
/// keyed `Model.instantiate` reads the same; an unknown name raises
/// `NameError` as `constantize` does. The bounded store evicts least-
/// recently-used entries, at Rails' entry cost, down to three quarters
/// of its size; `fetch` keeps the first value; Array keys file under
/// their joined form; reads are copies.
pub const RECORD_SNAPSHOT: Contract = Contract {
    path: "app/models/record_snapshot_probe.rb",
    source: r##"class RecordSnapshotProbe
  class DeleteAfterExistStore < ActiveSupport::Cache::MemoryStore
    # Deterministically model a delete in the gap between read_multi's
    # separate exist? and read calls. An atomic read_multi never calls
    # exist?, so it sees the entry before that competing delete.
    def exist?(name)
      found = super
      delete(name) if found
      found
    end
  end

  def self.round_trip
    created = Article.create!(title: "Snapshot", body: "A sufficiently long article body.")
    article = Article.find(created.id)
    raw = article.attributes_before_type_cast
    snapshot = ActiveSupport::JSON.encode([ [ article.class.name, raw ] ])
    rebuilt = ActiveSupport::JSON.decode(snapshot).map { |name, attributes| name.constantize.instantiate(attributes) }.first
    direct = Article.instantiate(raw.merge("title" => "Merged"))
    [ raw.keys.sort, rebuilt.persisted?, rebuilt.changed?, rebuilt.id == article.id, rebuilt.title,
      rebuilt.created_at == article.created_at, rebuilt.created_at.usec == article.created_at.usec,
      rebuilt.equal?(article), direct.title, direct.persisted? ]
  end

  def self.unknown_name
    attributes = { "id" => 1 }
    "NoSuchModel".constantize.instantiate(attributes)
    "built"
  rescue NameError
    "NameError"
  end

  def self.bounds
    store = ActiveSupport::Cache::MemoryStore.new(size: 1330)
    store.write("first", "a" * 100)
    store.write("second", "b" * 100)
    store.read("first")
    store.write("third", "c" * 400)
    [ store.exist?("first"), store.exist?("second"), store.exist?("third") ]
  end

  def self.fetching
    store = ActiveSupport::Cache::MemoryStore.new
    first = store.fetch("k") { "computed" }
    second = store.fetch("k") { "recomputed" }
    store.write([ "record", 1, nil, [ "a", "b" ] ], "filed")
    copies = !store.read("k").equal?(store.read("k"))
    store.delete("k")
    [ first, second, store.read("record/1//a/b"), copies, store.read("k").nil? ]
  end

  def self.read_multi_race
    store = DeleteAfterExistStore.new
    store.write("key", "cached")
    store.read_multi("key")["key"] == "cached"
  end

  def self.key
    ActiveSupport::Cache.expand_cache_key([ "record-snapshot-v1", [ "db", "ns", 3 ], [ "session", "abc" ] ])
  end
end
"##,
    script: concat!(
        "if ActiveRecord.adapter.nil?\n",
        "  SqliteAdapter.configure(\":memory:\")\n",
        "  ActiveRecord.adapter = SqliteAdapter\n",
        "  Schema.statements.each { |sql| SqliteAdapter.execute_ddl(sql) }\n",
        "end\n",
        r#"puts RecordSnapshotProbe.round_trip.inspect
puts RecordSnapshotProbe.unknown_name
puts RecordSnapshotProbe.bounds.inspect
puts RecordSnapshotProbe.fetching.inspect
puts RecordSnapshotProbe.read_multi_race
puts RecordSnapshotProbe.key
"#
    ),
    expected: concat!(
        "[[\"body\", \"created_at\", \"id\", \"title\", \"updated_at\"], true, false, true, \"Snapshot\", true, true, false, \"Merged\", true]\n",
        "NameError\n",
        "[true, false, true]\n",
        "[\"computed\", \"computed\", \"filed\", true, true]\n",
        "true\n",
        "record-snapshot-v1/db/ns/3/session/abc\n",
    ),
};

/// A controller overriding Rails' three fragment-caching hooks the way
/// campfire's `CachedResponses` does, each through `super`: caching off
/// for `?nocache=1`, its own bounded store, and an epoch in the key that
/// `POST /fragments/bump` moves. `GET /fragments/:id` caches the
/// article's title under `<% cache @article %>`.
pub fn fragments_overlay() -> Overlay {
    real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/fragments/:id\", to: \"fragments#show\"\n  post \"/fragments/bump\", to: \"fragments#bump\"\n",
        )
        .write(
            "app/controllers/fragments_controller.rb",
            r#"class FragmentsController < ApplicationController
  STORE = ActiveSupport::Cache::MemoryStore.new(size: 64 * 1024)
  EPOCH = [ 0 ]

  def perform_caching
    super && params[:nocache].nil?
  end

  def cache_store
    STORE
  end

  def combined_fragment_cache_key(key)
    super([ EPOCH.length, key ])
  end

  def show
    @article = Article.find(params[:id])
  end

  def bump
    EPOCH.push(EPOCH.length)
    head :ok
  end
end
"#,
        )
        .write(
            "app/views/fragments/show.html.erb",
            "<% cache @article do %><p><%= @article.title %></p><% end %>\n",
        )
}

/// `QrCodeController#show`'s rescue: data too long for any QR version
/// raises `RQRCodeCore::QRCodeRunTimeError`, answered 400; a short URL
/// renders its SVG. This overlay uses local `RQRCode`/`RQRCodeCore` test
/// doubles; the separate CI Spinel native-HTTP test exercises the package.
pub fn qr_code_overlay() -> Overlay {
    real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/qr/:size\", to: \"qr_codes#show\"\n",
        )
        .write(
            "app/controllers/qr_codes_controller.rb",
            r#"class QrCodesController < ApplicationController
  module RQRCodeCore
    class QRCodeRunTimeError < RuntimeError
    end
  end

  module RQRCode
    class QRCode
      def initialize(data)
        raise RQRCodeCore::QRCodeRunTimeError if data.length > 100
        @data = data
      end

      def as_svg(viewbox: false)
        "<svg>#{@data}</svg>"
      end
    end
  end

  def show
    svg = RQRCode::QRCode.new("https://example.com/" + ("x" * params[:size].to_i)).as_svg(viewbox: true)
    render plain: svg, content_type: "image/svg+xml"
  rescue ArgumentError, RQRCodeCore::QRCodeRunTimeError
    head :bad_request
  end
end
"#,
        )
}

/// A cache-through method shaped like campfire's `RecordCache.fetch`:
/// it answers its block's value on a miss and, on a hit, the records a
/// snapshot of that value rebuilds. Callers destructure the answer as
/// the block's own (`@membership, @room = RecordCache.fetch(…) { … }`),
/// so the analyzer types it as the block's value; both paths must then
/// hand back records of those classes, in that order. The snapshot path
/// intentionally trusts the cache-key invariant: each key is populated
/// only from the block result for that same key. A manually seeded or
/// colliding snapshot with a different shape is outside this contract.
pub const CACHE_THROUGH: Contract = Contract {
    path: "app/models/record_cache.rb",
    source: r##"class RecordCache
  STORE = ActiveSupport::Cache::MemoryStore.new

  def self.fetch(key)
    if snapshot = STORE.read(key)
      return ActiveSupport::JSON.decode(snapshot).map { |name, attributes| name.constantize.instantiate(attributes) }
    end

    records = yield
    STORE.write(key, ActiveSupport::JSON.encode(records.map { |record| [ record.class.name, record.attributes_before_type_cast ] }))
    records
  end

  def self.pair(id)
    article, comment = fetch("pair-#{id}") do
      found = Article.find(id)
      [ found, found.comments.first ]
    end
    [ article.title, comment.body, article.class.name, comment.class.name ]
  end

  def self.article_record(id)
    fetch("article-#{id}") { [ Article.find(id) ] }.first
  end

  def self.comment_record(id)
    fetch("comment-#{id}") { [ Comment.find(id) ] }.first
  end
end
"##,
    script: concat!(
        "if ActiveRecord.adapter.nil?\n",
        "  SqliteAdapter.configure(\":memory:\")\n",
        "  ActiveRecord.adapter = SqliteAdapter\n",
        "  Schema.statements.each { |sql| SqliteAdapter.execute_ddl(sql) }\n",
        "end\n",
        r#"article = Article.create!(title: "Cached", body: "A sufficiently long article body.")
Comment.create!(article_id: article.id, commenter: "Reader", body: "First comment")
puts RecordCache.pair(article.id).inspect
ActiveRecord::Base.connection.execute("UPDATE articles SET title = 'Renamed' WHERE id = #{article.id}")
puts RecordCache.pair(article.id).inspect
puts [ RecordCache.article_record(article.id).class.name, RecordCache.comment_record(article.id).class.name ].inspect
puts [ RecordCache.article_record(article.id).class.name, RecordCache.comment_record(article.id).class.name ].inspect
"#
    ),
    expected: concat!(
        "[\"Cached\", \"First comment\", \"Article\", \"Comment\"]\n",
        "[\"Cached\", \"First comment\", \"Article\", \"Comment\"]\n",
        "[\"Article\", \"Comment\"]\n",
        "[\"Article\", \"Comment\"]\n",
    ),
};

/// The caller's signature: typed through the cache-through method.
pub fn assert_cache_through_signature(rbs: &str) {
    // `title`/`body` are nullable columns, and `comments.first` may be nil.
    assert!(rbs.contains("def self.pair: (untyped id) -> [String?, String?, String, String]"), "{rbs}");
    assert!(rbs.contains("def self.article_record: (untyped id) -> Article"), "{rbs}");
    assert!(rbs.contains("def self.comment_record: (untyped id) -> Comment"), "{rbs}");
}
