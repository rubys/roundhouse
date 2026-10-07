# Primitive Db surface — the layer that per-model adapter code sits on
# top of. The contract is database-agnostic; this file is the SQLite-via-
# the-cruby-`sqlite3`-gem implementation. Other backends (sqlite via
# spinel FFI, postgres via libpq, etc.) implement the same `module Db`
# in sibling files; a future dispatcher picks one at require time. See
# project_level_3_adapter_emit.md.
#
# API (the contract every Db shim must satisfy):
#
#   Db.configure(path)         — open a database (":memory:" for tests)
#   Db.close                   — close the database
#   Db.exec(sql)               — run DDL / INSERT / UPDATE / DELETE
#   Db.prepare(sql)            — prepare a SELECT, returns stmt handle
#   Db.step?(stmt)             — advance, returns true if a row arrived
#   Db.column_int(stmt, i)     — read int column at zero-based index
#   Db.column_text(stmt, i)    — read text column at zero-based index
#   Db.column_count(stmt)      — number of columns in the prepared row
#   Db.column_name(stmt, i)    — name of column at zero-based index
#   Db.finalize(stmt)          — release the prepared stmt
#   Db.bind_int/bind_text/bind_bool(stmt, i, v) — bind `?` param i (1-based)
#   Db.last_insert_rowid       — id of the last INSERTed row
#   Db.changes                 — affected-row count of the last statement
#
# Stmt handles are opaque — under spinel FFI they're real `:ptr` values;
# under this CRuby shim each is a per-call Hash carrying the statement and
# its most recently stepped row (so column_int / column_text can pick
# fields by index, mirroring the FFI column accessors). Callers only ever
# hand it back to `Db.*`. It used to be an Integer indexing a process-wide
# table with a shared counter, which only CRuby's GVL made safe: Puma's
# worker threads run in parallel under JRuby and TruffleRuby, two of them
# drew the same id, and one request read the other's (finalized, nil)
# row. Like db_jruby.rb, there is now no shared mutable handle state.
#
# Per-database SQL dialect differences (placeholder syntax, RETURNING vs
# last_insert_rowid, etc.) live inside each shim or in a separate dialect
# helper consulted by the lowerer at SQL-composition time.
#
# The Db primitive surface backs the lowerer-emitted Level-3 per-model
# `_adapter_*` methods. This file is the CRuby (gem-backed) variant;
# `db.rb` in the same directory is the FFI variant the Spinel-AOT
# target compiles against.

require "sqlite3"
require "fileutils"

module Db
  @pool    = nil
  # WHAT PROCESS THIS POOL BELONGS TO, and what it would take to build
  # another one. Both exist for `fork`: see `adopt_after_fork`.
  @owner_pid = nil
  @path      = nil
  @pool_size = 0
  @mutex   = nil
  @cv      = nil
  # THE WRITE PERMIT: one writer at a time per process, handed out here
  # instead of by SQLite's busy handler. See `acquire_permit`.
  @permit_lock  = Mutex.new
  @permit_cv    = ConditionVariable.new
  @permit_owner = nil
  WRITE_PERMIT_TIMEOUT = 5.0
  @write_permit_timeout = WRITE_PERMIT_TIMEOUT
  # Background WAL checkpointing: asked for by the server boot
  # (`checkpoint_in_background!`), started per process on the first
  # lease. See `start_checkpointer`.
  @checkpoint_wanted = false
  @checkpointer_pid  = nil
  # The process-shared flock File while this process holds it. Retained
  # so `adopt_after_fork` can close the child's inherited copy without
  # LOCK_UN (parent keeps the lock). Cleared on release.
  @checkpoint_lock_file = nil
  # Failed resources stay reachable, but never re-enter the free list.
  @quarantined = []
  @missing_connections = 0
  # Per-connection prepared-statement cache bound (roundhouse#12). The
  # cache is LRU (hits re-insert; at cap the oldest entry is closed and
  # evicted), so a working set larger than the cap degrades gracefully
  # instead of pinning the first N statements forever and re-parsing
  # everything else — profiling the lobsters bench showed the old
  # first-come-stays policy spending 13% of wall time in
  # sqlite3_prepare/close because inlined id literals key per-id. The
  # cap covers the lobsters sequence's per-iteration distinct-SQL
  # working set with room; a prepared stmt is ~KBs, so worst case is a
  # few MB per connection.
  STMT_CACHE_CAP = 4096
  # Query-log capture (issue #27). `nil` ⇒ not capturing; an Array ⇒
  # accumulate the SQL each prepare/exec issues. The funnel hook
  # `record_query` is near-free (one nil check) when not capturing, so
  # this stays out of the way on the production path.
  @query_log = nil

  # Pool size defaults to the Puma thread count (RAILS_MAX_THREADS) so
  # every concurrently-serving thread can hold its own handle without
  # contending. Override explicitly for tests.
  def self.configure(path, pool_size: ENV.fetch("RAILS_MAX_THREADS", "3").to_i)
    @path      = path
    @pool_size = pool_size
    @mutex     = Mutex.new
    @cv        = ConditionVariable.new
    @quarantined = []
    @missing_connections = 0
    # Puma `before_worker_boot` re-configure sets @owner_pid to the child
    # and therefore skips `adopt_after_fork`. Drop any inherited
    # checkpoint-lock FD without LOCK_UN (same rule as adopt) and forget
    # the parent's checkpointer pid so the child's first lease starts a
    # fresh loop. Normal master boot never holds the flock; this closes
    # the gap if it ever did.
    inherited = @checkpoint_lock_file
    if inherited && !inherited.closed?
      begin
        inherited.close
      rescue StandardError
      end
    end
    @checkpoint_lock_file = nil
    @checkpointer_pid = nil
    @pool      = open_pool
    @owner_pid = Process.pid
  end

  # The pool construction, in one place because two callers need it: the
  # boot-time `configure` above and `adopt_after_fork` below.
  def self.open_pool
    ActiveRecord::ConnectionAdapters::ConnectionPool.new(@pool_size) { open_connection }
  end

  def self.open_connection
    path = @path
    # A `file:` URI (the test harness's shared-cache `file::memory:?
    # cache=shared`) needs the gem told to read it as one; a plain path
    # takes the gem's defaults, which are the same flags minus URI.
    flags = SQLite3::Constants::Open::READWRITE | SQLite3::Constants::Open::CREATE
    flags |= SQLite3::Constants::Open::URI if path.start_with?("file:")
    db = SQLite3::Database.new(path, flags: flags)
    db.results_as_hash = false
    # PINNED, not inherited. This lane reads `synchronous = NORMAL`
    # today without asking for it, because the sqlite3 gem's bundled
    # SQLite defaults WAL that way — while the binary's own SQLite
    # defaults to FULL, which cost the spinel lane 5.5s on a
    # 1,000-socket connect storm (one fsync per presence write; see
    # runtime/spinel/db.rb's PRAGMAS). Three lanes agreeing by
    # compile-time accident is not agreement, so each states it.
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("PRAGMA synchronous=NORMAL")
    # Page cache + mmap — same measured knobs as runtime/spinel/db.rb
    # (roundhouse#17 CRuby half). SQLite's default cache is 2 MB; a
    # long-lived serving process re-reads the working set from the OS
    # on every visit. Spinel measured ~400 pread64s per /top visit at
    # the default vs a large cache; tip CRuby still opened at
    # cache_size=-2000 / mmap_size=0. Negative cache_size is a KiB
    # budget (-65536 = 64 MiB); mmap_size maps the file so hits skip
    # the read() copy. Harmless on :memory: (pages are already heap).
    db.execute("PRAGMA cache_size=-65536")
    db.execute("PRAGMA mmap_size=268435456")
    # The gem's default is 0: a second writer fails at once with
    # SQLITE_BUSY. Rails' database.yml says `timeout: 5000`, and so do
    # the binary's PRAGMAS — the harness's file database (see
    # test/test_helper.rb) relies on writers waiting.
    #
    # The GVL-RELEASING handler, as Rails 8's adapter uses, not
    # `busy_timeout`: SQLite's own handler sleeps in C holding the GVL,
    # so a waiting writer stalls every other thread in the process —
    # including the one holding the lock it waits for. Writers inside
    # one process queue on the write permit (`exec`) and never reach
    # this; it is for a writer in ANOTHER process (a clustered Puma
    # sibling, a console, a migration).
    db.busy_handler_timeout = 5000
    # The app's SQL functions (`create_function` / `create_aggregate`
    # in an initializer), per connection as Rails' adapter registers
    # them. Defined only when the app has some (runtime/sql_functions.rb
    # is generated for it).
    SqlFunctions.install(db) if defined?(SqlFunctions)
    db
  end

  # A FORKED CHILD DOES NOT INHERIT A USABLE POOL, and the failure is
  # silent in the worst way.
  #
  # A clustered Puma boots the app in the master and then forks a worker
  # per `WEB_CONCURRENCY`, so every handle this pool opened belongs to
  # the parent. The sqlite3 gem notices — "Writable sqlite database
  # connection(s) were inherited from a forked process ... being closed
        # to prevent possible data corruption" — and discards them. It does
  # not tell the pool, which goes on handing the discarded handles out,
  # and a discarded handle does not raise: it answers every query
  # against an EMPTY SCHEMA. campfire's sign-in came back
  # `SQLite3::SQLException: no such table: bans` from a database whose
  # `bans` table is right there on disk.
  #
  # One Puma worker never forks, which is the only reason this survived
  # every benchmark run this tree has ever been in.
  #
  # The check is a pid comparison once per REQUEST (`with_connection`),
  # not once per query — `current_dbh` is the hot path and pays nothing
  # while a lease is held.
  def self.adopt_after_fork
    return if @pool.nil? || @owner_pid == Process.pid
    @mutex.synchronize do
      # Re-checked under the lock: every worker thread in a fresh child
      # reaches this together on the first request.
      if @owner_pid != Process.pid
        # Drop the inherited checkpoint-lock FD without LOCK_UN. The
        # parent may still hold the flock via its own descriptor; if we
        # unlocked here we would release the parent's hold. Closing the
        # child copy lets a surviving worker acquire after the parent
        # exits (Puma preload / clustered fork after checkpointer start).
        inherited = @checkpoint_lock_file
        if inherited && !inherited.closed?
          begin
            inherited.close
          rescue StandardError
          end
        end
        @checkpoint_lock_file = nil
        # The parent's handles are simply dropped. They are already
        # discarded by the gem's fork safety, and closing a descriptor
        # this process shares with its parent is not ours to do.
        @pool      = open_pool
        @quarantined = []
        @missing_connections = 0
        @owner_pid = Process.pid
      end
    end
  end

  # The SQLite3::Database this thread should read/write through. Set by
  # `with_connection` (request scope) when wired; falls back to the
  # pool's first free handle for single-thread test/dev modes.
  # `Fiber[:k]` is fiber-storage — under Puma's thread-per-request it is
  # effectively thread-local (each worker thread's root fiber).
  def self.current_dbh
    h = Fiber[:db_handle]
    return h if !h.nil?
    # Only the UNLEASED path pays the fork check — boot, tests, and
    # single-threaded dev. Under a lease the line above has already
    # returned, so the per-query cost of all this is zero.
    adopt_after_fork
    @pool.free[0]
  end

  # Request-scoped connection lease. Checks out a handle, binds it to
  # this thread's fiber-storage so `current_dbh` resolves to it for the
  # block's duration, and returns it on completion (even on raise).
  #
  # Thread-safe for the CRuby/Puma target where N worker threads share
  # one pool: the pool's free list (not itself thread-safe) is mutated
  # only under @mutex, and a thread parks on @cv when the pool is
  # momentarily exhausted (size < live requests) rather than raising.
  # With pool_size == thread count, the wait loop never trips.
  # Whether this thread is inside a `with_connection` lease. What
  # `Rails::Executor#wrap` asks before taking one: Rails' executor is
  # re-entrant, and a second lease on a thread that holds one would
  # rebind the connection and, on release, unbind the outer lease's.
  def self.in_lease?
    !Fiber[:db_handle].nil?
  end

  def self.with_connection
    h = nil
    adopt_after_fork
    @mutex.synchronize do
      while @pool.available_count == 0
        if @missing_connections > 0
          replace_connection
        else
          @cv.wait(@mutex)
        end
      end
      h = @pool.checkout
    end
    Fiber[:db_handle] = h
    request_failed = false
    begin
      prepare_for_checkpointer(h) if @checkpoint_wanted
      yield
    rescue Exception
      request_failed = true
      raise
    ensure
      cleanup_error = nil
      open = h.instance_variable_get(:@rh_open)
      begin
        if h.instance_variable_get(:@rh_snapshot_depth).to_i > 0
          h.instance_variable_set(:@rh_snapshot_depth, 1)
          read_snapshot_end
        end
      rescue StandardError => e
        cleanup_error = e
      end
      begin
        release_abandoned_write(h)
      rescue StandardError => e
        cleanup_error ||= e
      end
      begin
        release_open_statements(h) unless open.nil? || open.empty?
      rescue StandardError => e
        cleanup_error ||= e
      ensure
        Fiber[:db_handle] = nil
        begin
          if (open.nil? || open.empty?) && !h.transaction_active?
            @mutex.synchronize do
              @pool.checkin(h)
              @cv.signal
            end
          else
            quarantine_connection(h)
          end
        rescue StandardError => e
          cleanup_error ||= e
        end
      end
      raise cleanup_error if cleanup_error && !request_failed
    end
  end

  def self.quarantine_connection(conn)
    @mutex.synchronize do
      @quarantined << conn
      @missing_connections += 1
      begin
        # Open first so a shared in-memory database survives replacement.
        replace_connection
      ensure
        # Even a failed opener must wake waiters: they can retry or raise
        # its error, rather than wait forever for an impossible check-in.
        @cv.broadcast
      end
    end
    close_connection(conn)
  end

  # Called under @mutex. A failed open leaves the missing slot available
  # for the next exhausted checkout to retry, without touching healthy leases.
  def self.replace_connection
    replacement = open_connection
    @pool.checkin(replacement)
    @missing_connections -= 1
  end

  # Disposal is best-effort for every resource, retaining failed closes
  # for the next shutdown attempt. Return the first error to the caller.
  def self.close_connection(conn)
    error = nil
    begin
      release_open_statements(conn)
    rescue StandardError => e
      error = e
    end
    cache = conn.instance_variable_get(:@rh_stmt_cache)
    cache.each_value do |stmt|
      begin
        stmt.close unless stmt.closed?
      rescue StandardError => e
        error ||= e
      end
    end if cache
    begin
      conn.close unless conn.closed?
    rescue StandardError => e
      error ||= e
    end
    error
  end

  def self.close
    return if @pool.nil? && @quarantined.empty?
    conns = @quarantined.dup
    conns.concat(@pool.free) unless @pool.nil?
    error = nil
    conns.each do |conn|
      close_error = close_connection(conn)
      error ||= close_error
    end
    # A closed connection can still own idle statements whose close failed.
    @quarantined = conns.reject do |conn|
      cache = conn.instance_variable_get(:@rh_stmt_cache)
      conn.closed? && open_statements(conn).empty? &&
        (cache.nil? || cache.each_value.all?(&:closed?))
    end
    @pool = nil
    @owner_pid = nil
    raise error if error
  end

  def self.exec(sql)
    record_query(sql)
    # Any exec is (per the Db contract) DDL or a write — Rails
    # invalidates the whole query cache on write; so do we.
    qcache = Fiber[:rh_qcache]
    qcache.clear unless qcache.nil?
    conn = current_dbh
    # A write can't run inside the request's read snapshot: if another
    # connection committed since the snapshot began, SQLite refuses the
    # upgrade at once with SQLITE_BUSY (no busy handler is consulted).
    # End the snapshot first; the next read opens a fresh one that sees
    # this write.
    end_snapshot(conn)
    if permit_owned?
      # Inside this fiber's own transaction: the permit is already held.
      begin
        run_exec(conn, sql)
      ensure
        release_permit if sql == "ROLLBACK" || (sql == "COMMIT" && !conn.transaction_active?)
      end
    elsif conn.transaction_active?
      # Inside a transaction that began WITHOUT the permit (its wait
      # timed out — see `acquire_permit`): SQLite's lock is already
      # held, so there is nothing to queue for.
      run_exec(conn, sql)
    elsif sql == "BEGIN"
      # IMMEDIATE, as Rails 8's SQLite adapter begins: the write lock is
      # taken at BEGIN, not at the first write, so a transaction never
      # fails part way through upgrading a stale read.
      got = acquire_permit
      begin
        run_exec(conn, "BEGIN IMMEDIATE")
      rescue Exception
        release_permit if got
        raise
      end
    else
      got = acquire_permit
      begin
        run_exec(conn, sql)
      ensure
        release_permit if got
      end
    end
  end

  def self.run_exec(conn, sql)
    conn.execute(sql)
  rescue StandardError => e
    raise ActiveRecord::RecordNotUnique, e.message if Db.unique_violation?(e.message)
    raise
  end

  # ── The write permit ──
  #
  # SQLite allows one writer at a time. Left to itself, a second writer
  # gets SQLITE_BUSY and the busy handler retries on a timer, so writers
  # race: whoever retries at the right moment wins, and an unlucky one
  # loses again and again (the post-message tail). `exec` instead takes
  # the permit — for one statement in autocommit, or from BEGIN to
  # COMMIT/ROLLBACK — so writers in this process queue and go in turn.
  # The SQL still runs on the caller's own pooled connection; only the
  # permission is shared. A writer in another process still meets
  # SQLite's lock, and waits in `busy_handler_timeout`.
  #
  # The owner is a FIBER: BEGIN and COMMIT run on the same fiber, and a
  # different request on the same thread (Falcon) is a different fiber.
  #
  # THE WAIT IS BOUNDED. A transaction that starts a thread which writes
  # and then joins it would otherwise wait on itself forever. Before the
  # permit existed that case waited out SQLite's busy timeout and raised
  # SQLITE_BUSY; a hang is worse than that error. So after
  # `@write_permit_timeout` seconds a writer stops queueing and goes
  # straight to SQLite, which is exactly the old behaviour: it waits in
  # the busy handler and, if the lock never frees, raises BUSY.
  # Tests shorten the bound to exercise the timeout path quickly.
  def self.write_permit_timeout=(seconds)
    @write_permit_timeout = seconds
  end

  def self.acquire_permit
    fiber = Fiber.current
    @permit_lock.synchronize do
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + @write_permit_timeout
      # A dead owner (a killed thread) never releases; a Mutex would
      # have been freed with it, so this permit treats it as free too.
      until @permit_owner.nil? || !@permit_owner.alive?
        remaining = deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
        return false if remaining <= 0
        @permit_cv.wait(@permit_lock, remaining)
      end
      @permit_owner = fiber
    end
    true
  end

  def self.release_permit
    @permit_lock.synchronize do
      @permit_owner = nil
      @permit_cv.signal
    end
  end

  # Read without the lock: only the owning fiber ever sets the owner to
  # itself or clears it, so the answer for the CURRENT fiber is exact.
  def self.permit_owned?
    @permit_owner.equal?(Fiber.current)
  end

  def self.release_abandoned_write(conn)
    owned = permit_owned?
    return unless owned || conn.transaction_active?
    begin
      conn.execute("ROLLBACK") if conn.transaction_active?
    ensure
      release_permit if owned
    end
  end

  # ── The request read snapshot ──
  #
  # Outside a transaction every SELECT is its own transaction, and each
  # one sets up a WAL snapshot (shared lock on the WAL index, header
  # check). Inside an open transaction the same point query skips that:
  # measured 0.93 -> 0.36 us through this gem on an M-series Mac, and
  # the once-campfire Ruby port measured 4.5 -> 0.2 us on Linux. It also
  # gives the request one consistent view of the database, which
  # autocommit does not.
  #
  # The dispatcher brackets a GET/HEAD with `read_snapshot_begin` /
  # `read_snapshot_end`. Nothing happens until the first `prepare`
  # opens a deferred BEGIN; the first `exec` (any write) ends it, and the
  # read after that opens a new one. The app is never asked to promise
  # its GET handlers don't write. A request in a transaction of its own
  # is left alone: `prepare` opens a snapshot only when the connection
  # is in autocommit.
  #
  # States: nil (off), :wanted (on, not open), :open (BEGIN issued by
  # us). Brackets nest — a test that dispatches from inside a lease, a
  # job drained inside a request — and only the outermost end closes
  # the snapshot.
  #
  # The state lives ON THE CONNECTION, not in fiber storage. A snapshot
  # is a property of one SQLite connection, and a connection is leased
  # to one holder at a time. Fiber storage is copied into every Thread
  # created inside the request, so a thread spawned during a GET would
  # inherit `:open` and COMMIT a transaction its own connection never
  # began.
  def self.read_snapshot_begin
    conn  = current_dbh
    depth = conn.instance_variable_get(:@rh_snapshot_depth).to_i
    conn.instance_variable_set(:@rh_snapshot, :wanted) if depth == 0
    conn.instance_variable_set(:@rh_snapshot_depth, depth + 1)
  end

  def self.read_snapshot_end
    conn  = current_dbh
    depth = conn.instance_variable_get(:@rh_snapshot_depth).to_i - 1
    conn.instance_variable_set(:@rh_snapshot_depth, depth)
    return if depth > 0
    state = conn.instance_variable_get(:@rh_snapshot)
    conn.instance_variable_set(:@rh_snapshot, nil)
    conn.execute("COMMIT") if state == :open
  end

  def self.begin_snapshot(conn)
    return unless conn.instance_variable_get(:@rh_snapshot) == :wanted
    return if conn.transaction_active?
    conn.execute("BEGIN")
    conn.instance_variable_set(:@rh_snapshot, :open)
  end

  def self.end_snapshot(conn)
    return unless conn.instance_variable_get(:@rh_snapshot) == :open
    conn.execute("COMMIT")
    conn.instance_variable_set(:@rh_snapshot, :wanted)
  end

  # ── Checkpoints off the request path ──
  #
  # In WAL mode a write appends to the -wal file, and a checkpoint copies
  # it back into the database. SQLite's default runs one inside whichever
  # COMMIT pushes the log past 1,000 pages: that request pays for the
  # copy and an fsync (10-20 ms in the once-campfire port's measurements,
  # on about every 30th post — the p99). Under steady writes the log can
  # also never empty, because a checkpoint can only copy frames no reader
  # still needs.
  #
  # The server boot calls `checkpoint_in_background!`. Each serving
  # process then turns automatic checkpoints off on its pooled
  # connections and runs one thread with its own connection: PASSIVE
  # (copy what it can, wait on nobody) every CHECKPOINT_INTERVAL, and
  # RESTART once the log passes CHECKPOINT_RESTART_FRAMES, which makes
  # the next writer start the log from the beginning so the file stops
  # growing. RESTART holds the write permit, so in-process writers queue
  # behind it rather than meeting SQLITE_BUSY.
  #
  # Started from the first lease, not from boot, because a clustered
  # Puma boots the app in a master that forks and never serves: a thread
  # does not survive `fork`, and the master has no writes to checkpoint.
  # Tests, scripts and the console never ask, and keep SQLite's default.
  CHECKPOINT_INTERVAL = 0.25
  CHECKPOINT_RESTART_FRAMES = 8192 # ~32 MB of 4 KB pages
  # Cap failure noise: the loop keeps its 250 ms cadence (no Campfire-style
  # backoff that slows copying), but a stuck disk / permission / corruption
  # path must not stay completely silent either. One warn per interval.
  CHECKPOINT_WARN_INTERVAL = 30.0

  def self.checkpoint_in_background!
    @checkpoint_wanted = true
  end

  # Rate-limited visibility for checkpoint_loop failures. Resets the
  # suppress window only by time, not by success — a later success simply
  # stops calling this. Safe to call from tests.
  def self.warn_checkpoint_failure(error)
    now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    last = @checkpoint_warn_at
    return if last && (now - last) < CHECKPOINT_WARN_INTERVAL
    @checkpoint_warn_at = now
    warn "[db] WAL checkpoint failed: #{error.class}: #{error.message}"
  end

  def self.prepare_for_checkpointer(conn)
    start_checkpointer if @checkpointer_pid != Process.pid
    return if @checkpointer_pid != Process.pid
    return if conn.instance_variable_get(:@rh_manual_checkpoint)
    conn.execute("PRAGMA wal_autocheckpoint=0")
    conn.instance_variable_set(:@rh_manual_checkpoint, true)
  end

  def self.start_checkpointer
    @mutex.synchronize do
      return if @checkpointer_pid == Process.pid
      # Only a database file has a WAL to checkpoint.
      path = @path
      return if path.nil? || path == ":memory:" || path.start_with?("file:")
      @checkpointer_pid = Process.pid
      Thread.new { checkpoint_loop(path) }
    end
  end

  # One checkpointer across WEB_CONCURRENCY / Resque siblings (the bit
  # Campfire once-campfire#319 adds on top of the same PASSIVE loop).
  # Without a flock every forked worker runs its own 250ms PASSIVE and
  # they contend on the WAL; with it, losers skip the tick. Lock file
  # sits next to the database so each file-backed DB has its own.
  def self.checkpoint_lock_path(path)
    File.join(File.dirname(path), ".#{File.basename(path)}.wal_checkpoint.lock")
  end

  # File on acquire, `:busy` when another process holds the flock, `nil`
  # when the lock file cannot be used (mkdir/open/flock error). Callers
  # skip only on `:busy`; `nil` still checkpoints so a broken lock path
  # cannot disable WAL copy after `wal_autocheckpoint=0`.
  def self.try_checkpoint_lock(lock_path)
    file = nil
    FileUtils.mkdir_p(File.dirname(lock_path))
    file = File.open(lock_path, File::RDWR | File::CREAT, 0644)
    if file.flock(File::LOCK_EX | File::LOCK_NB)
      @checkpoint_lock_file = file
      return file
    end
    file.close
    :busy
  rescue StandardError
    begin
      file.close if file && !file.closed?
    rescue StandardError
    end
    nil
  end

  def self.release_checkpoint_lock(file)
    return if file.nil? || !file.is_a?(File)
    begin
      file.flock(File::LOCK_UN)
    ensure
      file.close
      @checkpoint_lock_file = nil if @checkpoint_lock_file.equal?(file)
    end
  rescue StandardError
  end

  def self.checkpoint_loop(path)
    lock_path = checkpoint_lock_path(path)
    # Open can fail (permissions, missing file mid-deploy). Do not let
    # the thread die after prepare_for_checkpointer already set
    # wal_autocheckpoint=0 — warn and retry on the same cadence.
    conn = nil
    until conn
      begin
        conn = SQLite3::Database.new(path)
        conn.busy_handler_timeout = 100
      rescue StandardError => error
        warn_checkpoint_failure(error)
        sleep CHECKPOINT_INTERVAL
      end
    end
    loop do
      # Hold the flock for the whole inner loop (Campfire #319), not
      # per tick: releasing every 250ms lets a sibling overlap a
      # PASSIVE with ours. Losers sleep and retry; a dead winner
      # drops the flock so another worker takes over.
      lock = try_checkpoint_lock(lock_path)
      if lock == :busy
        sleep CHECKPOINT_INTERVAL
        next
      end
      held_error = nil
      begin
        loop do
          sleep CHECKPOINT_INTERVAL
          row = conn.execute("PRAGMA wal_checkpoint(PASSIVE)")[0]
          log_frames = row.nil? ? 0 : row[1].to_i
          if log_frames >= CHECKPOINT_RESTART_FRAMES
            if acquire_permit
              begin
                conn.execute("PRAGMA wal_checkpoint(RESTART)")
              ensure
                release_permit
              end
            end
          end
        end
      rescue StandardError => error
        held_error = error
      ensure
        # Release before warn so a blocked $stderr cannot extend the
        # exclusive flock and delay sibling takeover.
        release_checkpoint_lock(lock)
      end
      # Retry on the next outer pass (same 250 ms cadence). Warn at
      # most once per CHECKPOINT_WARN_INTERVAL so a persistent failure
      # is visible without a multi-contender log storm.
      warn_checkpoint_failure(held_error) if held_error
    end
  end

  # A UNIQUE-index violation is `ActiveRecord::RecordNotUnique`, not
  # whatever this driver raises. Rails' contract is what apps write
  # against — campfire's sign-up rescues it to turn a lost race into a
  # redirect to the login screen, and its first-run screen does the same
  # for two people opening a brand-new install at once. Without the
  # mapping the rescue never matched and the raw driver error reached
  # the dispatcher as a 500.
  #
  # THE TEST IS SQLITE'S OWN MESSAGE, not the driver's exception class,
  # and that is deliberate: "UNIQUE constraint failed: users.
  # email_address" comes out of the engine, so the same string appears
  # in the cruby gem's ConstraintException, in the JDBC SQLException and
  # in `sqlite3_errmsg` under spinel. One rule, three drivers, no
  # per-driver class table to keep in step. (The strict targets carry
  # their own `Db` and their own mapping; this is the ruby-family half.)
  def self.unique_violation?(message)
    message.to_s.include?("UNIQUE constraint failed")
  end

  # Prepared-statement cache (roundhouse#12). A SQLite3::Statement is bound
  # to the connection it was prepared on, so the cache lives ON the
  # connection object — and since `with_connection` leases a connection to
  # exactly one thread at a time, the per-connection cache needs no extra
  # lock. A cache hit rewinds the stmt (`reset!`) instead of re-parsing the
  # SQL; `finalize` resets rather than closes, so the stmt stays cached
  # (real `close` runs at pool shutdown). Key is the composed SQL: inlined
  # literals mean id-bearing queries key per-id (fine for the bench;
  # STMT_CACHE_CAP bounds growth, beyond which statements are transient and
  # closed on finalize). With placeholder binding on (roundhouse#12) the
  # key is instead the static shape (`WHERE id = ?`), so id-varying
  # queries share one cached statement.
  # ── per-request SQL query cache (Rails AR query-cache semantics) ──
  # Identical SELECTs within one request replay the first result set;
  # any `exec` (writes ride exec per the Db contract) invalidates.
  # Fiber-local so Puma's thread-per-request can't cross-pollute.
  # Entries capture rows AS CONSUMED plus an eof flag: a point-lookup
  # reader that steps once caches one row + eof=false, and a later
  # consumer wanting more rows promotes to a real re-executed
  # statement (rare). Enabled per-request by the dispatch; nil ⇒ off
  # (tests, scripts) with a single fiber-storage read of overhead.
  QC_CAPTURE_ROWS = 16

  def self.query_cache_begin
    Fiber[:rh_qcache] = {}
  end

  def self.query_cache_end
    Fiber[:rh_qcache] = nil
  end

  # Statement identity => owning handle, on the leased connection. This
  # protects both cache hits and eviction, starting before the first step.
  def self.open_statements(conn)
    open = conn.instance_variable_get(:@rh_open)
    if open.nil?
      open = {}.compare_by_identity
      conn.instance_variable_set(:@rh_open, open)
    end
    open
  end

  # Abandoned handles must not publish a partial replay capture. Called
  # before checkin (including exceptions) and before pool shutdown.
  def self.release_open_statements(conn)
    open = conn.instance_variable_get(:@rh_open)
    error = nil
    open.each_value do |entry|
      begin
        release_statement(entry, conn)
      rescue StandardError => e
        error ||= e
      end
    end unless open.nil?
    raise error if error
  end

  def self.prepare(sql)
    # A `?`-bearing SQL string is a placeholder query (roundhouse#12):
    # its result depends on the runtime binds set AFTER prepare, which
    # aren't in the SQL key — so it must NOT participate in the
    # result-replay query cache (replaying would serve one bind value's
    # rows for another). The prepared-statement cache below still keys on
    # the shared shape, which is the whole point. (Heuristic: a literal
    # value containing `?` would also skip qcache — a safe miss, never a
    # wrong result.)
    parameterized = sql.include?("?")
    qcache = Fiber[:rh_qcache]
    if !qcache.nil? && !parameterized && (hit = qcache[sql])
      # A replay is not a round trip, and `capture_sql` does not count
      # it — the same rule as Rails' SQLCounter, which skips CACHE
      # events, and as the spinel lane's `Db.prepare`.
      return { stmt: nil, row: nil, cached: false, replay: hit, pos: 0, sql: sql }
    end
    record_query(sql)
    conn  = current_dbh
    begin_snapshot(conn)
    cache = conn.instance_variable_get(:@rh_stmt_cache)
    if cache.nil?
      cache = {}
      conn.instance_variable_set(:@rh_stmt_cache, cache)
    end
    open = conn.instance_variable_get(:@rh_open) || open_statements(conn)
    stmt   = cache[sql]
    cached = true
    if !stmt.nil? && open.key?(stmt)
      # A nested reader of this shape needs its own cursor and bindings.
      stmt = conn.prepare(sql)
      cached = false
    elsif stmt.nil?
      stmt = conn.prepare(sql)
      if cache.size >= STMT_CACHE_CAP
        # Evict least-recently-used (Ruby Hash is insertion-ordered and
        # hits below re-insert, so the earliest key is the LRU). Skip
        # any statement still held by an open handle — closing it
        # under a live cursor would break nested prepare patterns. If
        # everything is somehow in use, insert past the cap (soft
        # bound) rather than close a live statement.
        live = nil
        cache.each_key do |k|
          candidate = cache[k]
          next if open.key?(candidate)
          live = k
          break
        end
        unless live.nil?
          evicted = cache.delete(live)
          begin
            evicted.close
          rescue StandardError
            # The fresh prepare already succeeded. Own both resources
            # before propagating the eviction error so lease cleanup can
            # close the fresh statement and quarantine a failed evictee.
            open[evicted] = { stmt: evicted, row: nil, cached: false, capture: nil, open: open } unless evicted.closed?
            open[stmt] = { stmt: stmt, row: nil, cached: false, capture: nil, open: open } unless stmt.closed?
            raise
          end
        end
      end
      cache[sql] = stmt
    else
      # Only a successful release makes a statement idle: it is already
      # reset and unbound. Re-insert to record recency (LRU discipline).
      cache.delete(sql)
      cache[sql] = stmt
    end
    handle = { stmt: stmt, row: nil, cached: cached, capture: nil, open: open }
    open[stmt] = handle
    handle[:capture] = { rows: [], names: stmt.columns, eof: false, sql: sql } if !qcache.nil? && !parameterized
    handle
  end

  # Explicit uncached reads skip statement reuse, but the separate
  # request result cache still applies. Partial replays already promote
  # to a transient statement, which finalize closes.
  def self.prepare_uncached(sql)
    qcache = Fiber[:rh_qcache]
    parameterized = sql.include?("?")
    if !qcache.nil? && !parameterized && (hit = qcache[sql])
      return { stmt: nil, row: nil, cached: false, replay: hit, pos: 0, sql: sql }
    end
    record_query(sql)
    conn = current_dbh
    begin_snapshot(conn)
    stmt = conn.prepare(sql)
    statement_handle(open_statements(conn), stmt, sql, false, !qcache.nil? && !parameterized)
  end

  # Transient handles use the same ownership and bounded capture contract.
  # The cached path constructs its handle inline on the query hot path.
  def self.statement_handle(open, stmt, sql, cached, capture_rows)
    handle = { stmt: stmt, row: nil, cached: cached, capture: nil, open: open }
    open[stmt] = handle
    handle[:capture] = { rows: [], names: stmt.columns, eof: false, sql: sql } if capture_rows
    handle
  end

  def self.step?(handle)
    entry = handle
    if (hit = entry[:replay])
      if entry[:pos] < hit[:rows].length
        entry[:row] = hit[:rows][entry[:pos]]
        entry[:pos] += 1
        return true
      end
      return false if hit[:eof]
      # Cached prefix exhausted without eof (original consumer stopped
      # early) — promote to a real transient statement, fast-forwarded
      # past the rows already replayed.
      conn = current_dbh
      stmt = conn.prepare(entry[:sql])
      entry[:stmt] = stmt
      entry[:replay] = nil
      entry[:promoted] = true
      entry[:open] = open_statements(conn)
      entry[:open][stmt] = entry
      entry[:pos].times { stmt.step }
      row = stmt.step
      entry[:row] = row
      return !row.nil?
    end
    row = entry[:stmt].step
    entry[:row] = row
    if (c = entry[:capture])
      if row.nil?
        c[:eof] = true
      elsif c[:rows].length >= QC_CAPTURE_ROWS
        # Bounded like the spinel shim's (db.rb, `QC_CAPTURE_ROWS`): the
        # cache is for the point lookups a page repeats, and a result
        # past the bound is not kept — the same rows come back from a
        # re-execution rather than a replay.
        entry[:capture] = nil
      else
        c[:rows] << row
      end
    end
    !row.nil?
  rescue StandardError => e
    statement_failed(entry, "step", e)
  end

  def self.statement_failed(entry, _operation, error)
    entry[:closed] = true
    entry[:replay] = nil
    begin
      release_statement(entry)
    rescue StandardError
      # Failed closes remain owned until lease cleanup quarantines them.
    end
    raise error
  end

  def self.column_int(handle, i)
    handle[:row][i].to_i
  end

  def self.column_float(handle, i)
    handle[:row][i].to_f
  end

  def self.column_text(handle, i)
    v = handle[:row][i]
    v.nil? ? "" : v.to_s
  end

  # Nullable-column reads. A column the schema declares nullable holds
  # NULL until something sets it, and NULL is not the type's zero:
  # `column_text` collapsing it to "" makes a nullable UNIQUE column
  # collide row-to-row, and `column_int`'s 0 makes `where(fk: nil)`
  # match nothing. These are the reads the lowerer emits for those
  # columns; the non-`_opt` readers stay exactly as they were for
  # NOT NULL columns, which is most of them.
  def self.column_int_opt(handle, i)
    v = handle[:row][i]
    v.nil? ? nil : v.to_i
  end

  def self.column_float_opt(handle, i)
    v = handle[:row][i]
    v.nil? ? nil : v.to_f
  end

  def self.column_text_opt(handle, i)
    v = handle[:row][i]
    v.nil? ? nil : v.to_s
  end

  def self.column_bool_opt(handle, i)
    v = handle[:row][i]
    v.nil? ? nil : v.to_i != 0
  end

  # Raw typed column read: the value exactly as the sqlite3 gem
  # returns it — Integer for INTEGER affinity, Float for REAL, String
  # for TEXT, and crucially nil for NULL (column_text collapses NULL
  # to "", which breaks Rails semantics like `group_by(&:fk)[nil]`
  # and integer-column truthiness). Whole-row hydration
  # (SqliteAdapter.select_rows) reads through this so model attributes
  # carry real types, matching what ActiveRecord hands the app.
  def self.column_value(handle, i)
    handle[:row][i]
  end

  def self.column_count(handle)
    e = handle
    return e[:replay][:names].length if e[:replay]
    e[:stmt].columns.length
  end

  def self.column_name(handle, i)
    e = handle
    return e[:replay][:names][i] if e[:replay]
    e[:stmt].columns[i]
  end

  # Release the per-call handle. A cached stmt is reset! (rewound + read
  # lock dropped) and kept for reuse; a transient (over-cap or
  # replay-promoted) stmt is closed; a pure replay handle held no
  # statement at all. A capture is published to the request's query
  # cache on release — even a partial one (eof=false): the next
  # identical SELECT replays the consumed prefix and promotes past it
  # only if it wants more.
  def self.finalize(entry)
    return if entry.nil? || entry[:stmt].nil?
    if (c = entry[:capture])
      qcache = Fiber[:rh_qcache]
      qcache[c[:sql]] = c if !qcache.nil? && !qcache.key?(c[:sql])
    end
    release_statement(entry)
  end

  def self.release_statement(entry, conn = nil)
    stmt = entry[:stmt]
    return if stmt.nil?
    error = nil
    if entry[:cached]
      begin
        stmt.reset!
      rescue StandardError => e
        error = e
      end
      begin
        stmt.clear_bindings!
      rescue StandardError => e
        error ||= e
      end
      if error
        conn ||= current_dbh
        cache = conn.instance_variable_get(:@rh_stmt_cache)
        cache.delete_if { |_sql, cached| cached.equal?(stmt) } if cache
        entry[:cached] = false
      end
    end
    unless entry[:cached]
      begin
        stmt.close
      rescue StandardError => e
        error ||= e
      end
    end
    # A failed close still owns its native cursor. Keep the handle in the
    # connection's open set so lease cleanup can quarantine it.
    if error.nil? || stmt.closed?
      entry[:open].delete(stmt)
      entry[:stmt] = nil
    end
    entry[:capture] = nil
    raise error if error
  end

  # The gem checks SQLite return codes. Failed binds must also abandon
  # captures and release checkouts when rescued within a lease.
  def self.bind_value(handle, idx, value)
    raise "statement is not bindable" if handle[:stmt].nil?
    handle[:stmt].bind_param(idx, value)
    nil
  rescue StandardError => e
    statement_failed(handle, "bind", e)
  end

  def self.bind_int(handle, idx, value)
    bind_value(handle, idx, value)
  end

  def self.bind_int_opt(handle, idx, value)
    bind_int(handle, idx, value)
  end

  def self.bind_text_opt(handle, idx, value)
    value.nil? ? bind_value(handle, idx, nil) : bind_text(handle, idx, value)
  end

  def self.bind_bool_opt(handle, idx, value)
    bind_value(handle, idx, value.nil? ? nil : (value ? 1 : 0))
  end

  def self.bind_text(handle, idx, value)
    value = value.to_s
    # Match escape_string's storage class exactly. The gem otherwise binds
    # every BINARY string as BLOB and every UTF-8 string (even NUL) as TEXT;
    # SQLite equality does not equate the same bytes across those classes.
    if value.include?("\0") || (value.encoding == Encoding::BINARY && !value.ascii_only?)
      bind_value(handle, idx, value.b)
    elsif value.encoding == Encoding::BINARY
      bind_value(handle, idx, value.encode(Encoding::UTF_8))
    else
      bind_value(handle, idx, value)
    end
  rescue StandardError => error
    # Encoding checks/conversion can raise before bind_value is entered.
    statement_failed(handle, "bind", error)
  end

  def self.bind_bool(handle, idx, value)
    bind_value(handle, idx, value.nil? ? nil : (value ? 1 : 0))
  end

  def self.last_insert_rowid
    current_dbh.last_insert_row_id
  end

  def self.changes
    current_dbh.changes
  end

  # Query-log capture — the test-side analog of Rails'
  # `ActiveSupport::Notifications.subscribed(counter, "sql.active_record")`
  # (activerecord testing/query_assertions.rb). Records the SQL every
  # prepare/exec issues during the block and returns it as an Array of
  # SQL strings, the shape Rails' `capture_sql` yields. Nestable: an
  # outer capture is restored on exit. Production never calls this, so
  # the funnel hook stays a single nil check off the hot path.
  #
  # The only instrument that can see the `includes(:assoc)` N+1:
  # byte-identical `compare` is blind to it (eager-load and N+1 render
  # the same HTML; only the query strategy differs). See issue #27.
  def self.capture_sql
    prev = @query_log
    log = []
    @query_log = log
    begin
      yield
    ensure
      @query_log = prev
    end
    log
  end

  # Funnel hook: record one SQL string into the active capture, if any.
  # No-op (single nil check) when no capture is installed.
  def self.record_query(sql)
    @query_log.push(sql) unless @query_log.nil?
  end

  # SQL-value escaping primitives — lowerer-emitted code uses these to
  # inline literals (and runtime values when the placeholder-bind gate is
  # off). With the gate on, runtime values instead flow through `bind_*`
  # above; both shims (this gem-backed one and spinel-FFI) now support
  # binding. Inlining stays safe regardless since the lowerer controls
  # every string that flows here.
  # BYTES go out as a hex BLOB literal, `X'…'`. A NUL cannot ride a
  # quoted literal at all (it ends the SQL text: "unrecognized token"),
  # and a binary value stored as TEXT sorts before every BLOB, so a
  # `t.binary` column filled half one way and half the other orders
  # wrong. Bytes = BINARY-encoded and not plain ASCII (lobsters'
  # `[a, b, c].pack("CCC")` confidence_order), or any string holding a
  # NUL. An ASCII-only BINARY string stays text, as it always was.
  def self.escape_string(s)
    str = s.to_s
    if str.include?("\0") || (str.encoding == Encoding::BINARY && !str.ascii_only?)
      return "X'" + str.unpack1("H*") + "'"
    end
    "'" + str.gsub("'", "''") + "'"
  end

  def self.escape_int(n)
    n.to_i.to_s
  end

  # Nullable-column writes: nil renders the SQL keyword NULL rather
  # than `''` / `0`, so what the schema calls nullable round-trips as
  # nil instead of coming back as the type's zero.
  def self.escape_string_opt(s)
    s.nil? ? "NULL" : escape_string(s)
  end

  def self.escape_int_opt(n)
    n.nil? ? "NULL" : escape_int(n)
  end

  def self.escape_float_opt(f)
    f.nil? ? "NULL" : f.to_f.to_s
  end

  def self.escape_bool_opt(b)
    b.nil? ? "NULL" : escape_bool(b)
  end

  # Render an integer list for `IN (...)` eager-load batches (issue
  # #27). Empty list → "NULL" so `IN (NULL)` is valid SQL matching no
  # rows (an empty `IN ()` is a syntax error).
  def self.escape_int_list(ids)
    return "NULL" if ids.empty?

    ids.map { |i| i.to_i.to_s }.join(", ")
  end

  # SQLite stores booleans as 0/1 integers (no native bool type) —
  # AR `t.boolean :col` maps to INTEGER affinity. Emit the inline
  # literal directly; saves a CAST round-trip vs `'true'`/`'false'`.
  def self.escape_bool(b)
    b ? "1" : "0"
  end

  # Read a boolean column. SQLite returns 0/1 (integer), we widen to
  # Ruby's bool. Nulls coerce to false.
  def self.column_bool(handle, idx)
    column_int(handle, idx) != 0
  end
end
