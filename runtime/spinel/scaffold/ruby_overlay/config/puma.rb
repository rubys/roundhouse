# Puma configuration — mirrors Rails 7.1+ generator output so
# benchmarks against a baseline Rails app run under identical
# server configuration.
#
# Single-mode (workers = 0) is the default for clarity in
# benchmarking; flip to clustered with `WEB_CONCURRENCY=N`.
# Threads default to 3 per worker (Rails 7.1 generator default);
# override with `RAILS_MAX_THREADS=5` for the IO-heavy bench.

threads_count = ENV.fetch("RAILS_MAX_THREADS", 3).to_i
threads threads_count, threads_count

port ENV.fetch("PORT", 3000)
environment ENV.fetch("RAILS_ENV", "development")

# Clustered mode — enable via WEB_CONCURRENCY=N. Defaults to single
# process for the bench baseline.
workers ENV.fetch("WEB_CONCURRENCY", 0).to_i

# `preload_app!` is required for clustered mode + copy-on-write
# memory sharing. Single-mode ignores it.
preload_app! if ENV.fetch("WEB_CONCURRENCY", "0").to_i > 0

# Re-open the SQLite connection per forked worker. `preload_app!`
# opens the DB in the master; workers inherit a file descriptor that
# SQLite cannot reuse safely post-fork. Re-running Db.configure here
# gives each worker its own handle to the same file. Puma 8 renamed
# `on_worker_boot` to `before_worker_boot`.
#
# THE PATH COMES FROM THE APP, not from a second copy of its default.
# This line used to read `ENV.fetch("BLOG_DB", ":memory:")`, and the
# app's own default is `storage/development.sqlite3`. With one worker
# the app loads inside the worker and fixes it up; with two or more,
# `preload_app!` loads the app in the MASTER, so this hook was the last
# word and every worker served from a fresh EMPTY in-memory database.
# Every query answered `no such table: <anything>` — not an error the
# pool could raise, just an empty schema — and campfire's sign-in 500'd
# on `bans` while the file on disk had the table all along.
before_worker_boot do
  if defined?(Main) && Main.respond_to?(:default_db_path)
    Db.configure(Main.default_db_path) if defined?(Db)
  elsif defined?(Db)
    Db.configure(ENV.fetch("BLOG_DB", "storage/development.sqlite3"))
  end
end

# `touch tmp/restart.txt` to restart workers without dropping
# connections. Rails generator includes this by default.
plugin :tmp_restart
