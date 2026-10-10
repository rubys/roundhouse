# Primitive Db surface over PostgreSQL, for spinel-compiled binaries.
#
# The same `module Db` contract as runtime/spinel/db.rb (SQLite over
# libsqlite3 FFI) and its gem-backed siblings, typed in
# runtime/ruby/db.rbs, implemented on the spinel-pg driver
# (https://github.com/rubys/spinel-pg): a pure spinel-Ruby PostgreSQL
# client over Spinel's own sp_net sockets, so the program links no
# libpq. A raw compile finds it with `spinel -I <spinel-pg checkout>`;
# a spin package lists `pg` in `[dependencies]`.
#
# Nothing selects this file yet. The `--database postgres` wiring for
# `--target spinel` (shipping this file as runtime/db.rb, the `pg`
# dependency, PostgreSQL DDL at boot) follows the shared database
# selection; until then tests/spinel_pg_db.rs compiles it directly.
#
# What maps onto what:
#
#   Db.configure(url)    — `postgres://user:pass@host:port/db`, with the
#                          libpq PG* environment variables as fallbacks.
#                          No TLS, unix sockets or query parameters; a
#                          URL that asks for them is refused.
#   Db.prepare(sql)      — a statement handle (an Integer) on the current
#                          connection. SQL spells its placeholders $1, $2…
#   Db.bind_*(h, i, v)   — one-based, as with SQLite; values travel as
#                          text (to_s, "t"/"f", nil as NULL).
#   Db.step?(h)          — the first call runs the statement with its
#                          bound values (the extended protocol's unnamed
#                          statement) and buffers the rows; each call
#                          then advances one row.
#   Db.column_*(h, i)    — zero-based reads, converted from PostgreSQL's
#                          text format.
#   Db.finalize(h)       — releases the handle.
#   Db.exec(sql)         — simple query; `changes` comes from its tag.
#
# Writes: `exec`, and `exec_returning` for a write with a RETURNING
# clause, which answers a handle over the returned rows. Server errors
# with a SQLSTATE that ActiveRecord names are raised as that class
# (pg_errors.rb); others surface as the driver's PG::Error. A
# transaction keeps its connection: inside a lease that is the leased
# one, and a BEGIN outside a lease pins the connection to the thread
# until COMMIT or ROLLBACK. A lease that ends with a transaction still
# open (or failed) rolls it back before the connection is reused, and a
# connection whose session ended is reopened on next use.
#
# Pooling follows db.rb: DATABASE_POOL_SIZE connections split into
# shards, a thread assigned one shard for its life, connections opened
# lazily. `prepare` runs through a named statement cached per physical
# connection (Parse once, then Bind/Execute), bounded and least recently
# used, closed on the server when evicted; `prepare_uncached` uses the
# unnamed statement. Finalizing a read releases its handle, never the
# named statement. Not here yet: the request query cache. The
# SQLite-only entry points the server boot calls are no-ops, except
# `seed_from_file`, which raises (see below).
#
# The SQL-functions hook: project.rs installs an app's SQLite functions
# by patching a connection-open anchor in runtime/db.rb. This file does
# not carry that anchor on purpose; SQLite user functions have no
# PostgreSQL equivalent, so an app that registers them is refused rather
# than served without them.
#
# Spinel idioms, as in db.rb: instance-ivar arrays of one element type,
# Integer handles, assign-then-return, and `rescue PG::Error => e`
# (typed) before reading anything from a driver error.
require "pg"
require_relative "pg_errors"

# Connection settings from a URL, then the libpq PG* environment
# variables, then libpq's defaults (localhost:5432, database named after
# the user).
class PgConfig
  def initialize(url)
    @host = PgUrl.env("PGHOST", "localhost")
    @port = PgUrl.env("PGPORT", "5432").to_i
    @user = PgUrl.env("PGUSER", PgUrl.env("USER", "postgres"))
    @password = PgUrl.env("PGPASSWORD", "")
    @database = PgUrl.env("PGDATABASE", "")
    parse(url) if url != ""
    @database = @user if @database == ""
  end

  def host
    @host
  end

  def port
    @port
  end

  def user
    @user
  end

  def password
    @password
  end

  def database
    @database
  end

  # `scheme://[user[:password]@][host][:port][/database]`. Every part is
  # optional; an empty one keeps its fallback.
  def parse(url)
    rest = ""
    if url.start_with?("postgres://")
      rest = url[11, url.length - 11]
    elsif url.start_with?("postgresql://")
      rest = url[13, url.length - 13]
    else
      raise "Db.configure: expected a postgres:// or postgresql:// URL"
    end
    if rest.include?("?")
      raise "Db.configure: URL parameters (sslmode and the like) are not supported"
    end
    authority = rest
    slash = rest.index("/")
    if !slash.nil?
      authority = rest[0, slash]
      path = rest[slash + 1, rest.length - slash - 1]
      @database = PgUrl.decode(path) if path != ""
    end
    hostport = authority
    at = authority.rindex("@")
    if !at.nil?
      userinfo = authority[0, at]
      hostport = authority[at + 1, authority.length - at - 1]
      colon = userinfo.index(":")
      if colon.nil?
        @user = PgUrl.decode(userinfo) if userinfo != ""
      else
        u = userinfo[0, colon]
        @user = PgUrl.decode(u) if u != ""
        @password = PgUrl.decode(userinfo[colon + 1, userinfo.length - colon - 1])
      end
    end
    if hostport.start_with?("[")
      raise "Db.configure: bracketed IPv6 hosts are not supported"
    end
    colon = hostport.rindex(":")
    if colon.nil?
      @host = hostport if hostport != ""
    else
      h = hostport[0, colon]
      @host = h if h != ""
      p = hostport[colon + 1, hostport.length - colon - 1]
      @port = p.to_i if p != ""
    end
    nil
  end
end

module PgUrl
  def self.env(name, fallback)
    v = ENV.fetch(name, "")
    v == "" ? fallback : v
  end

  # Percent-decoding for the user, password and database parts.
  def self.decode(s)
    return s if !s.include?("%")
    out = ""
    i = 0
    n = s.bytesize
    while i < n
      b = s.getbyte(i)
      if b == 37 && i + 2 < n
        hi = PgUrl.hex(s.getbyte(i + 1))
        lo = PgUrl.hex(s.getbyte(i + 2))
        if hi >= 0 && lo >= 0
          out = out + (hi * 16 + lo).chr
          i += 3
          next
        end
      end
      out = out + b.chr
      i += 1
    end
    out
  end

  def self.hex(b)
    return b - 48 if b >= 48 && b <= 57
    return b - 55 if b >= 65 && b <= 70
    return b - 87 if b >= 97 && b <= 102
    -1
  end
end

# One prepared read: the SQL, the values bound so far (text, with a
# parallel null-flag array so the element type stays String), and once
# it has run, the buffered result and the current row.
class PgStmt
  def initialize(handle, sql, cached)
    @handle = handle
    @sql = sql
    @cached = cached
    @vals = [""]
    @vals.delete_at(0)
    @nulls = [0]
    @nulls.delete_at(0)
    @result = nil
    @ran = false
    @row = -1
    @alive = true
  end

  def handle
    @handle
  end

  def alive
    @alive
  end

  def kill
    @alive = false
    nil
  end

  def sql
    @sql
  end

  # Whether the read runs through the connection's named statement.
  def cached
    @cached
  end

  def ran
    @ran
  end

  # One-based, as the contract (and SQLite) number placeholders. A slot
  # never bound is sent as NULL, which is also SQLite's rule.
  def bind(idx, value)
    if idx < 1
      raise "Db.bind: parameter index " + idx.to_s + " is not one-based"
    end
    if @ran
      raise "Db.bind: statement already ran; prepare it again to rebind"
    end
    while @vals.length < idx
      @vals.push("")
      @nulls.push(1)
    end
    if value.nil?
      @vals[idx - 1] = ""
      @nulls[idx - 1] = 1
    else
      @vals[idx - 1] = value
      @nulls[idx - 1] = 0
    end
    nil
  end

  def params
    out = [""]
    out.delete_at(0)
    i = 0
    while i < @vals.length
      if @nulls[i] == 1
        out.push(nil)
      else
        out.push(@vals[i])
      end
      i += 1
    end
    out
  end

  def finish(result)
    @result = result
    @ran = true
    @row = -1
    nil
  end

  def result
    r = @result
    raise "Db: statement has not run" if r.nil?
    r
  end

  def advance
    r = result
    n = r.ntuples
    @row = @row + 1 if @row < n
    @row < n
  end

  def value(i)
    r = result
    if @row < 0 || @row >= r.ntuples
      raise "Db.column: no current row (step? first, and only while it returns true)"
    end
    if i < 0 || i >= r.nfields
      raise "Db.column: column index " + i.to_s + " out of range"
    end
    v = r.getvalue(@row, i)
    v
  end
end

# One cached named statement: the SQL and its server-side name.
class PgNamed
  def initialize(sql, name)
    @sql = sql
    @name = name
  end

  def sql
    @sql
  end

  def name
    @name
  end
end

# One server session, opened on first use. Owns the handles prepared on
# it until they are finalized or the lease that took it ends.
#
# A handle is `(serial << 24) + slot`: the slot finds the statement in
# one index, and the serial makes a finalized handle fail loudly instead
# of reaching whichever statement reused its slot. The table is only
# indexed and pushed (never shrunk), so it stays a typed PgStmt array;
# freed slots are kept for reuse.
class PgConn
  SLOT_BITS = 24
  SLOT_MASK = 16777215

  def initialize(config)
    @config = config
    @client = nil
    @stmts = []
    @free_slots = [0]
    @free_slots.delete_at(0)
    @serial = 0
    @changes = 0
    @last_tag = ""
    @last_insert_table = ""
    # Named statements, least recently used first.
    @named = []
    @name_serial = 0
  end

  def client
    c = @client
    if c.nil?
      cfg = @config
      c = PG.connect(cfg.host, cfg.port, cfg.database, cfg.user, cfg.password)
      @client = c
    end
    c
  end

  def changes
    @changes
  end

  def last_tag
    @last_tag
  end

  def open_stmt(sql, cached)
    @serial = @serial + 1
    slot = @stmts.length
    if @free_slots.length > 0
      slot = @free_slots.delete_at(@free_slots.length - 1)
    end
    h = (@serial << SLOT_BITS) + slot
    st = PgStmt.new(h, sql, cached)
    if slot == @stmts.length
      @stmts.push(st)
    else
      @stmts[slot] = st
    end
    h
  end

  # The live statement for `h`, or -1.
  def slot_of(h)
    slot = h & SLOT_MASK
    return -1 if h <= 0 || slot >= @stmts.length
    st = @stmts[slot]
    return -1 if !st.alive || st.handle != h
    slot
  end

  def stmt(h)
    slot = slot_of(h)
    raise "Db: unknown or finalized statement handle " + h.to_s if slot < 0
    @stmts[slot]
  end

  def close_stmt(h)
    slot = slot_of(h)
    return nil if slot < 0
    @stmts[slot].kill
    @free_slots.push(slot)
    nil
  end

  # Results are buffered client-side, so a handle holds no server
  # resource; marking every slot free is the whole release.
  def release_all
    @free_slots.clear
    i = @stmts.length - 1
    while i >= 0
      @stmts[i].kill
      @free_slots.push(i)
      i -= 1
    end
    nil
  end

  # Statements still open on this connection.
  def open_count
    @stmts.length - @free_slots.length
  end

  def run(st)
    begin
      if st.cached
        st.finish(run_named(st))
      else
        st.finish(client.exec_params(st.sql, st.params))
      end
    rescue PG::Error => e
      PgErrors.raise_mapped(e.result.error_field(PG::PG_DIAG_SQLSTATE).to_s, e.message)
      raise e
    end
    nil
  end

  # ── Named statements ──

  def named_count
    @named.length
  end

  # Whether `sql` has a named statement on this session.
  def named?(sql)
    named_index(sql) >= 0
  end

  def named_index(sql)
    i = @named.length - 1
    while i >= 0
      return i if @named[i].sql == sql
      i -= 1
    end
    -1
  end

  # The server-side name for `sql`, parsed now if this session has none.
  # A hit moves to the most-recent end; a miss past the bound closes the
  # least recently used statement on the server.
  def named_for(sql)
    i = named_index(sql)
    if i >= 0
      e = @named[i]
      last = @named.length - 1
      while i < last
        @named[i] = @named[i + 1]
        i += 1
      end
      @named[last] = e
      return e.name
    end
    @name_serial = @name_serial + 1
    name = "rh_s" + @name_serial.to_s
    client.prepare(name, sql)
    @named.push(PgNamed.new(sql, name))
    evict_over(Db.statement_cache_cap)
    name
  end

  # Close the oldest statements until at most `cap` remain. A Close the
  # server refuses (an aborted transaction) still forgets the entry; the
  # statement then lives until the session ends.
  def evict_over(cap)
    while @named.length > cap
      e = @named[0]
      keep = []
      i = 1
      while i < @named.length
        keep.push(@named[i])
        i += 1
      end
      @named = keep
      begin
        client.close_prepared(e.name)
      rescue PG::Error
        nil
      end
    end
    nil
  end

  def forget_named(sql)
    i = named_index(sql)
    return nil if i < 0
    keep = []
    j = 0
    while j < @named.length
      keep.push(@named[j]) if j != i
      j += 1
    end
    @named = keep
    nil
  end

  # Bind/Execute the cached statement. If the server no longer has it
  # (26000, e.g. after DEALLOCATE ALL) or its plan went stale after DDL
  # (0A000), forget it and, outside a transaction, parse it again and
  # retry once. Inside one the error has already failed the transaction,
  # so it is raised; the next use after ROLLBACK parses afresh.
  def run_named(st)
    name = named_for(st.sql)
    begin
      r = client.exec_prepared(name, st.params)
      return r
    rescue PG::Error => e
      code = e.result.error_field(PG::PG_DIAG_SQLSTATE).to_s
      raise e if code != "26000" && code != "0A000"
      forget_named(st.sql)
      if code == "0A000"
        begin
          client.close_prepared(name)
        rescue PG::Error
          nil
        end
      end
      raise e if client.transaction_status != PG::PQTRANS_IDLE
    end
    name = named_for(st.sql)
    r2 = client.exec_prepared(name, st.params)
    r2
  end

  def exec(sql)
    simple(sql)
    nil
  end

  # A simple query: the result, with its row count and INSERT target
  # recorded for `changes` and `last_insert_rowid`.
  def simple(sql)
    begin
      r = client.exec(sql)
      @last_tag = r.cmd_tag
      @changes = Db.tag_count(@last_tag)
      @last_insert_table = @last_tag.start_with?("INSERT ") ? Db.insert_target(sql) : ""
      return r
    rescue PG::Error => e
      PgErrors.raise_mapped(e.result.error_field(PG::PG_DIAG_SQLSTATE).to_s, e.message)
      raise e
    end
  end

  # A handle over a write's returned rows, already in hand.
  def returning(sql)
    r = simple(sql)
    h = open_stmt(sql, false)
    stmt(h).finish(r)
    h
  end

  # From the last ReadyForQuery; UNKNOWN before the first connect and
  # after the session ended.
  def status
    c = @client
    return PG::PQTRANS_IDLE if c.nil?
    c.transaction_status
  end

  # Lease-end hygiene: roll back a transaction the holder left open or
  # failed, and drop a session that ended (FATAL, lost transport) so the
  # next use reconnects. Never raises; a failed ROLLBACK drops the session.
  def reset_for_reuse
    st = status
    if st == PG::PQTRANS_INTRANS || st == PG::PQTRANS_INERROR
      begin
        client.exec("ROLLBACK")
      rescue StandardError
        drop
      end
    elsif st == PG::PQTRANS_UNKNOWN
      drop
    end
    nil
  end

  # Forget the session without the Terminate handshake.
  def drop
    c = @client
    @client = nil
    @named = []
    release_all
    begin
      c.close if !c.nil?
    rescue StandardError
      nil
    end
    nil
  end

  def last_insert_table
    @last_insert_table
  end

  # The key the last INSERT on this session drew from `table`'s own
  # sequence: `currval` of the sequence behind its single-column primary
  # key. Another table's sequence never answers, and a key with no
  # sequence (a uuid, a natural key) raises.
  def inserted_key(table)
    lit = Db.escape_string(table)
    sql = "SELECT currval(pg_get_serial_sequence(" + lit + ", (SELECT a.attname " +
          "FROM pg_index i JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0] " +
          "WHERE i.indrelid = " + lit + "::regclass AND i.indisprimary AND i.indnatts = 1)))"
    v = ""
    begin
      r = client.exec(sql)
      s = r.getvalue(0, 0)
      v = s if !s.nil?
    rescue PG::Error => e
      raise "Db.last_insert_rowid: no key from " + table + "'s sequence (" + e.message + ")"
    end
    if v == ""
      raise "Db.last_insert_rowid: " + table + " has no serial or identity primary key; " +
            "only those are supported here"
    end
    v.to_i
  end

  def close
    c = @client
    @client = nil
    @named = []
    release_all
    c.close if !c.nil?
    nil
  end
end

# One shard of the pool: connections opened lazily, leased per request.
class PgPool
  def initialize(config, n)
    @conns = []
    @free = []
    @lock = Mutex.new
    @cv = ConditionVariable.new
    i = 0
    while i < n
      @conns.push(PgConn.new(config))
      @free.push(i)
      i += 1
    end
  end

  def lease
    idx = 0
    @lock.synchronize do
      while @free.length == 0
        @cv.wait(@lock)
      end
      idx = @free.delete_at(@free.length - 1)
    end
    idx
  end

  def release(idx)
    @lock.synchronize do
      @free.push(idx)
      @cv.signal
    end
    nil
  end

  def conn(idx)
    @conns[idx]
  end

  def first
    @conns[0]
  end

  def close_all
    error = nil
    i = 0
    while i < @conns.length
      begin
        @conns[i].close
      rescue StandardError => e
        error = e if error.nil?
      end
      i += 1
    end
    raise error if !error.nil?
    nil
  end
end

module Db
  # Shards (see configure); empty until configured.
  @pools = []
  @database = ""
  @assign_lock = Mutex.new
  @next_pool = 0
  # Named statements kept per connection (db.rb's DbConn::CAP).
  @statement_cache_cap = 128
  # Query-log capture (issue #27), in parity with db.rb.
  @query_log = nil
  # RH_SQL_TRACE=1 prints each SQL string to stderr as `  SQL ...`.
  @sql_trace = false

  # Pool size: kwarg, overridden by DATABASE_POOL_SIZE as in db.rb. The
  # first connection opens here, so a wrong URL or password fails at
  # boot rather than on the first request.
  def self.configure(url, pool_size: 8)
    @sql_trace = ENV.fetch("RH_SQL_TRACE", "") != ""
    n = pool_size
    ev = ENV["DATABASE_POOL_SIZE"]
    if !ev.nil? && ev != ""
      n = ev.to_i
    end
    n = 1 if n < 1
    # Sharded as db.rb's pool is, for the same reason: one lock per
    # thread rather than per request. At least 4 connections a shard,
    # at most 8 shards.
    stripes = n / 4
    stripes = 1 if stripes < 1
    stripes = 8 if stripes > 8
    per = n / stripes
    per = 1 if per < 1
    config = PgConfig.new(url.to_s)
    @database = config.database
    pools = []
    i = 0
    while i < stripes
      pools.push(PgPool.new(config, per))
      i += 1
    end
    pools[0].first.client
    @pools = pools
    @next_pool = 0
    nil
  end

  def self.statement_cache_cap
    @statement_cache_cap
  end

  # Tests shrink the bound to exercise eviction.
  def self.statement_cache_cap=(n)
    @statement_cache_cap = n
  end

  # The first shard: where scripts and boot run outside any lease.
  def self.pool
    raise "Db: not configured (call Db.configure first)" if @pools.empty?
    @pools[0]
  end

  def self.shard_count
    @pools.length
  end

  # The shard this thread leases from, chosen once (round-robin) and
  # remembered, as in db.rb.
  def self.pool_for_thread
    raise "Db: not configured (call Db.configure first)" if @pools.empty?
    pi = Thread.current[:db_pg_pool]
    return @pools[pi] if !pi.nil? && pi < @pools.length
    n = 0
    @assign_lock.synchronize do
      n = @next_pool
      @next_pool = n + 1
    end
    pi = n % @pools.length
    Thread.current[:db_pg_pool] = pi
    @pools[pi]
  end

  # The leased connection, or the pool's first one for single-threaded
  # scripts, boot and tests, as in db.rb.
  def self.current_conn
    c = Thread.current[:db_conn]
    return c if !c.nil?
    Db.pool.first
  end

  # `ActiveRecord::Base.connection_db_config` answers from these
  # (runtime/spinel/active_record_db_config.rb): the database this
  # process configured, the adapter name Rails would report for it, and
  # whether the holder is inside a transaction of its own — the request
  # read snapshot is the shim's, not the app's, so it does not count.
  def self.database_path
    @database
  end

  def self.adapter_name
    "postgresql"
  end

  def self.transaction_open?
    st = current_conn.status
    st == PG::PQTRANS_INTRANS || st == PG::PQTRANS_INERROR
  end

  def self.in_lease?
    !Thread.current[:db_conn].nil?
  end

  # Request-scoped lease. Handles left open by the block are released
  # with the lease, and the release runs even when the block raises.
  def self.with_connection
    # Re-entrant: a nested lease, or one inside a transaction a BEGIN
    # outside any lease pinned, keeps the thread's connection.
    return yield if !Thread.current[:db_conn].nil?
    pool = Db.pool_for_thread
    idx = pool.lease
    conn = pool.conn(idx)
    Thread.current[:db_conn] = conn
    request_error = nil
    begin
      result = yield
    rescue Exception => e
      request_error = e
    ensure
      cleanup_error = nil
      begin
        conn.release_all
      rescue StandardError => e
        cleanup_error = e
      ensure
        conn.reset_for_reuse
        Thread.current[:db_conn] = nil
        pool.release(idx)
      end
      raise cleanup_error if !cleanup_error.nil? && request_error.nil?
    end
    raise request_error if !request_error.nil?
    result
  end

  def self.close
    pools = @pools
    return nil if pools.empty?
    @pools = []
    error = nil
    i = 0
    while i < pools.length
      begin
        pools[i].close_all
      rescue StandardError => e
        error = e if error.nil?
      end
      i += 1
    end
    raise error if !error.nil?
    nil
  end

  # ── SQLite-only entry points ──
  #
  # The server boot and dispatcher call these for SQLite's sake. On
  # PostgreSQL they have nothing to do:
  #
  # - The read snapshot is a BEGIN around a GET so SQLite's readers see
  #   one WAL state. Rails' PostgreSQL adapter opens no transaction for
  #   reads, and READ COMMITTED gives each statement its own snapshot.
  # - The checkpointer keeps SQLite's WAL short; the server checkpoints
  #   its own WAL.
  # - The request query cache is a later step here; until then every
  #   read is a round trip.
  def self.read_snapshot_begin
    true
  end

  def self.read_snapshot_end
    true
  end

  def self.checkpoint_in_background!
    nil
  end

  def self.query_cache_begin
    nil
  end

  def self.query_cache_end
    nil
  end

  def self.query_cache_enabled?
    false
  end

  # A page-level copy of a SQLite file has no PostgreSQL meaning. Seed
  # with pg_restore or psql before boot instead.
  def self.seed_from_file(src_path)
    raise "Db.seed_from_file(" + src_path.to_s + "): copies a SQLite database file; " +
          "on PostgreSQL, seed with pg_restore or psql before boot"
  end

  # ── Statements ──

  def self.exec(sql)
    record_query(sql)
    conn = current_conn
    conn.exec(sql)
    Db.pin_transaction(conn)
    # A value, not nil, for the same reason as db.rb: `result = yield`
    # in with_connection cannot hold a void.
    true
  end

  # A BEGIN outside a lease binds its connection to this thread until
  # the transaction ends, so a lease taken inside it (with_connection)
  # writes through the same session instead of committing on another.
  def self.pin_transaction(conn)
    if conn.status == PG::PQTRANS_IDLE
      Thread.current[:db_conn] = nil if Thread.current[:db_txn_pin] == true
      Thread.current[:db_txn_pin] = false
    elsif !Db.in_lease?
      Thread.current[:db_conn] = conn
      Thread.current[:db_txn_pin] = true
    end
    nil
  end

  # A write with a RETURNING clause (roundhouse#91): runs once, inline,
  # like `exec`, and answers a handle over the returned rows
  # (step?/column_*/finalize, as for a read). `changes` is its row count.
  # There is no query cache to invalidate yet.
  def self.exec_returning(sql)
    record_query(sql)
    conn = current_conn
    h = conn.returning(sql)
    Db.pin_transaction(conn)
    h
  end

  # A read through the connection's cached named statement for `sql`.
  def self.prepare(sql)
    sql = sql.to_s
    record_query(sql)
    h = current_conn.open_stmt(sql, true)
    h
  end

  # Reads with too many SQL shapes to cache run as the unnamed statement.
  def self.prepare_uncached(sql)
    sql = sql.to_s
    record_query(sql)
    h = current_conn.open_stmt(sql, false)
    h
  end

  def self.step?(stmt)
    conn = current_conn
    st = conn.stmt(stmt)
    conn.run(st) if !st.ran
    st.advance
  end

  def self.finalize(stmt)
    current_conn.close_stmt(stmt)
    nil
  end

  # Run a statement that has not stepped yet, for the column metadata
  # SQLite knows at prepare time and PostgreSQL only from a result.
  def self.ran_stmt(stmt)
    conn = current_conn
    st = conn.stmt(stmt)
    conn.run(st) if !st.ran
    st
  end

  def self.column_count(stmt)
    Db.ran_stmt(stmt).result.nfields
  end

  def self.column_name(stmt, i)
    r = Db.ran_stmt(stmt).result
    if i < 0 || i >= r.nfields
      raise "Db.column_name: column index " + i.to_s + " out of range"
    end
    r.fields[i]
  end

  def self.cell(stmt, i)
    v = current_conn.stmt(stmt).value(i)
    v
  end

  # ── Reads (zero-based) ──
  #
  # PostgreSQL sends text: integers as digits, floats as digits with an
  # optional exponent, booleans as "t"/"f", timestamps as
  # "YYYY-MM-DD HH:MM:SS[.ffffff]" (ActiveSupport.parse_db_time's input).

  def self.column_int(stmt, i)
    v = Db.cell(stmt, i)
    return 0 if v.nil?
    Db.int_value(v)
  end

  def self.column_float(stmt, i)
    v = Db.cell(stmt, i)
    return 0.0 if v.nil?
    v.to_f
  end

  def self.column_text(stmt, i)
    v = Db.cell(stmt, i)
    return "" if v.nil?
    v
  end

  def self.column_bool(stmt, i)
    v = Db.cell(stmt, i)
    return false if v.nil?
    Db.bool_value(v)
  end

  def self.column_int_opt(stmt, i)
    v = Db.cell(stmt, i)
    return nil if v.nil?
    Db.int_value(v)
  end

  def self.column_float_opt(stmt, i)
    v = Db.cell(stmt, i)
    return nil if v.nil?
    v.to_f
  end

  def self.column_text_opt(stmt, i)
    v = Db.cell(stmt, i)
    v
  end

  def self.column_bool_opt(stmt, i)
    v = Db.cell(stmt, i)
    return nil if v.nil?
    Db.bool_value(v)
  end

  # The driver's value as the SQLite shim hands it to the row-hash path
  # (sqlite_adapter.rb): Integer, Float, String or nil. Dispatches on the
  # column's type OID. A boolean reads as 1/0, which is how SQLite
  # stores one and what the emitted hydration already accepts.
  def self.column_value(stmt, i)
    st = current_conn.stmt(stmt)
    v = st.value(i)
    return nil if v.nil?
    oid = st.result.ftype(i)
    if oid == 20 || oid == 21 || oid == 23 || oid == 26
      v.to_i
    elsif oid == 700 || oid == 701 || oid == 1700
      v.to_f
    elsif oid == 16
      v == "t" ? 1 : 0
    else
      v
    end
  end

  # A bool column read through column_int answers 1/0 as SQLite's does.
  def self.int_value(v)
    return 1 if v == "t"
    return 0 if v == "f"
    v.to_i
  end

  def self.bool_value(v)
    return true if v == "t"
    return false if v == "f"
    v.to_i != 0
  end

  # ── Binds (one-based) ──

  def self.bind_int(stmt, idx, value)
    bind_int_opt(stmt, idx, value)
  end

  def self.bind_int_opt(stmt, idx, value)
    current_conn.stmt(stmt).bind(idx, value.nil? ? nil : value.to_s)
  end

  def self.bind_text(stmt, idx, value)
    current_conn.stmt(stmt).bind(idx, value.to_s)
  end

  def self.bind_text_opt(stmt, idx, value)
    current_conn.stmt(stmt).bind(idx, value)
  end

  def self.bind_bool(stmt, idx, value)
    bind_bool_opt(stmt, idx, value)
  end

  def self.bind_bool_opt(stmt, idx, value)
    current_conn.stmt(stmt).bind(idx, value.nil? ? nil : (value ? "t" : "f"))
  end

  # ── Write results ──

  # The row count from the last exec's command tag ("INSERT 0 2",
  # "UPDATE 3", "DELETE 0"); 0 for tags without one ("CREATE TABLE").
  def self.changes
    current_conn.changes
  end

  def self.tag_count(tag)
    parts = tag.split(" ")
    return 0 if parts.empty?
    parts[parts.length - 1].to_i
  end

  # The key of the row the previous INSERT on this connection created,
  # read from that table's own sequence (see PgConn#inserted_key). Only
  # serial and identity keys qualify; anything else raises, as does a
  # previous statement that was not an INSERT.
  def self.last_insert_rowid
    conn = current_conn
    if !conn.last_tag.start_with?("INSERT ")
      raise "Db.last_insert_rowid: the previous statement on this connection was not an INSERT"
    end
    table = conn.last_insert_table
    if table == ""
      raise "Db.last_insert_rowid: cannot read the table name from the last INSERT"
    end
    conn.inserted_key(table)
  end

  # The table an `INSERT INTO <table>` names, as written: an identifier,
  # quoted or bare, optionally schema-qualified. "" when the SQL is not
  # that shape.
  def self.insert_target(sql)
    n = sql.bytesize
    i = 0
    while i < n && Db.space_byte?(sql.getbyte(i))
      i += 1
    end
    return "" if n - i < 12 || sql[i, 12].upcase != "INSERT INTO "
    i += 12
    while i < n && Db.space_byte?(sql.getbyte(i))
      i += 1
    end
    start = i
    parts = 0
    while parts < 2
      stop = Db.identifier_end(sql, i)
      return "" if stop == i
      i = stop
      parts += 1
      break if i >= n || sql.getbyte(i) != 46
      i += 1
    end
    sql[start, i - start]
  end

  # Where the identifier starting at `i` ends: a double-quoted one (with
  # "" escapes) or a bare run of letters, digits, `_` and `$`.
  def self.identifier_end(sql, i)
    n = sql.bytesize
    return i if i >= n
    if sql.getbyte(i) == 34
      j = i + 1
      while j < n
        if sql.getbyte(j) == 34
          return j + 1 if j + 1 >= n || sql.getbyte(j + 1) != 34
          j += 2
        else
          j += 1
        end
      end
      return i
    end
    j = i
    while j < n
      b = sql.getbyte(j)
      ok = (b >= 97 && b <= 122) || (b >= 65 && b <= 90) || (b >= 48 && b <= 57) || b == 95 || b == 36 || b >= 128
      break if !ok
      j += 1
    end
    j
  end

  def self.space_byte?(b)
    b == 32 || b == 10 || b == 9 || b == 13
  end

  # ── Query-log capture (issue #27), as in db.rb ──

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

  def self.record_query(sql)
    @query_log.push(sql) unless @query_log.nil?
    if @sql_trace
      $stderr.puts "  SQL " + sql
    end
  end

  def self.sql_trace?
    @sql_trace
  end

  # ── Escaping for inlined values ──
  #
  # Quotes double, as standard_conforming_strings (on by default since
  # PostgreSQL 9.1) reads them; backslashes are literal.

  def self.escape_string(s)
    "'" + s.to_s.gsub("'", "''") + "'"
  end

  def self.escape_int(n)
    n.to_i.to_s
  end

  def self.escape_bool(b)
    b ? "TRUE" : "FALSE"
  end

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

  # `IN (NULL)` matches no rows, where `IN ()` is a syntax error.
  def self.escape_int_list(ids)
    return "NULL" if ids.empty?

    ids.map { |i| i.to_i.to_s }.join(", ")
  end
end

# Temporal intrinsics, chained off Db as db.rb chains them.
require_relative "active_support_time_parsing"
