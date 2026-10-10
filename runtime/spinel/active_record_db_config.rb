# Which database this process is connected to, and whether it is inside
# a transaction — the Rails API campfire's SQLite-observer caches read
# (rubys/roundhouse#698), answered from the Db shim the tree loaded
# (`Db.database_path` / `adapter_name` / `transaction_open?`, in each of
# db.rb, db_cruby.rb, db_jruby.rb and db_pg.rb).
#
# - `ActiveRecord::Base.connection_db_config` — `.database` and
#   `.adapter`. `ResponseCache` opens a read-only observer on that file;
#   `SqliteWalCheckpoint` checkpoints it.
# - `ActiveRecord::Base.connection_pool.with_connection { |c| … }` and
#   `connection.transaction_open?` — `FragmentCache.transaction_open?`,
#   which keeps a page rendered inside a transaction out of the cache.
#   The transaction is the app's: the request read snapshot the shims
#   open around a GET does not count, as Rails has no such thing.
# - `ActiveRecord::Base.instantiate_named(name, attributes)` — what
#   `lower::record_snapshot` makes of `name.constantize.instantiate(...)`.
# - `ActiveRecord::ConnectionAdapters::SQLite3Adapter.resolve_path` —
#   Rails main's (activerecord/lib/active_record/connection_adapters/
#   sqlite3_adapter.rb): a `file:` URI's path, expanded against the app
#   root. Written without URI: `URI.parse(db).path` for `file:/…` is the
#   part after the authority, up to the query; `URI.parse(db.split("?")
#   .first).opaque` for `file:x` is what follows `file:`.
#
# Ruby family and spinel only (every Db shim lives under runtime/spinel/
# and ships to both); a strict target is refused at the call
# (`project::RUBY_SPINEL_ONLY_METHODS`).
module ActiveRecord
  module DatabaseConfigurations
    class HashConfig
      def initialize(env_name, name, database, adapter)
        @env_name = env_name
        @name = name
        @database = database
        @adapter = adapter
      end

      def env_name
        @env_name
      end

      def name
        @name
      end

      def database
        @database
      end

      def adapter
        @adapter
      end
    end
  end

  module ConnectionAdapters
    # The face of the Db shim's own pool: Rails' `with_connection` runs
    # the block with the holder's connection, leasing one only when it
    # holds none, which is what `ActiveRecord::Base.connection` (a
    # stateless facade over `Db`) already does.
    class DbPool
      def with_connection
        yield ActiveRecord::Base.connection
      end

      # Rails hands the thread's connection back to the pool so another
      # can take it — campfire's counter test does, before writing
      # through a second, foreign `SQLite3::Database`. Ours holds no
      # lock between statements, so there is nothing to hand back.
      def release_connection
        nil
      end
    end

    class SQLite3Adapter
      def self.resolve_path(database, root: nil)
        text = database.to_s
        path = text
        if text.start_with?("file:/")
          rest = text[5, text.length - 5]
          if rest.start_with?("//")
            slash = rest.index("/", 2)
            rest = slash.nil? ? "" : rest[slash, rest.length - slash]
          end
          path = rest.split("?").first.to_s.split("#").first.to_s
        elsif text.start_with?("file:")
          head = text.split("?").first.to_s
          path = head[5, head.length - 5]
        end
        base = root.nil? ? Rails.root.to_s : root.to_s
        base.empty? ? path : File.expand_path(path, base)
      end
    end
  end

  class Connection
    def transaction_open?
      Db.transaction_open?
    end
  end

  class Base
    def self.connection_db_config
      env = Rails.env_name.to_s
      env = "development" if env.empty?
      ActiveRecord::DatabaseConfigurations::HashConfig.new(env, "primary", Db.database_path, Db.adapter_name)
    end

    def self.connection_pool
      ActiveRecord::ConnectionAdapters::DbPool.new
    end

    # `name.constantize.instantiate(attributes)` (`lower::record_snapshot`):
    # the app's models are a closed set, so the class is a `case` over
    # their names rather than a constant computed from a String. Each arm
    # is that model's own `instantiate` (String-keyed, as Rails'); an
    # unknown name raises as `constantize` does. The arms are written per
    # app by `project::apply_instantiate_named`.
    def self.instantiate_named(name, attributes)
      # >>> generated: instantiate-named
      raise NameError, "uninitialized constant #{name}"
      # <<< generated: instantiate-named
    end
  end
end
