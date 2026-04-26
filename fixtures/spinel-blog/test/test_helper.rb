require "minitest/autorun"

ROOT = File.expand_path("..", __dir__)
$LOAD_PATH.unshift(File.join(ROOT, "runtime"))
$LOAD_PATH.unshift(File.join(ROOT, "app"))
$LOAD_PATH.unshift(File.join(ROOT, "config"))

require "sqlite_adapter"
require "active_record"
require "schema"

# One-time global setup: configure the adapter against an in-memory
# SQLite database, load the schema, and wire ActiveRecord.adapter to
# point at it. Per-test isolation comes from `SchemaSetup.reset!` —
# called from each test class's `setup` block — which truncates the
# tables but leaves the schema intact.
SqliteAdapter.configure(":memory:")
Schema.load!(SqliteAdapter)
ActiveRecord.adapter = SqliteAdapter

module SchemaSetup
  module_function

  TABLES = %w[articles comments].freeze

  def reset!
    TABLES.each do |t|
      SqliteAdapter.db.execute("DELETE FROM #{t}")
      SqliteAdapter.db.execute("DELETE FROM sqlite_sequence WHERE name = ?", [t])
    end
  end
end
