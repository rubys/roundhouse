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

# `touch tmp/restart.txt` to restart workers without dropping
# connections. Rails generator includes this by default.
plugin :tmp_restart
