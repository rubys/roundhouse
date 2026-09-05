# Primitive Db surface — the layer that per-model adapter code sits on
# top of. The contract is database-agnostic; this file is the
# SQLite-via-libsqlite3-FFI implementation, the runtime spinel-compiled
# binaries use. The sqlite3-gem-backed sibling (`db_cruby.rb`) is the
# stock-CRuby implementation; both define `module Db` with the same
# external API. main.rb requires this file (the FFI variant);
# test_helper.rb requires `db_cruby` (the gem variant) so the existing
# `ruby -Itest test/...` developer loop keeps working under stock CRuby.
#
# API (the contract every Db shim must satisfy — must match db_cruby.rb):
#
#   Db.configure(path)         — open a database (":memory:" for tests)
#   Db.close                   — close the database
#   Db.exec(sql)               — run DDL / INSERT / UPDATE / DELETE
#   Db.prepare(sql)            — prepare a SELECT, returns stmt handle
#   Db.step?(stmt)             — advance, returns true if a row arrived
#   Db.column_int(stmt, i)     — read int column at zero-based index
#   Db.column_float(stmt, i)   — read float (REAL) column at zero-based index
#   Db.column_text(stmt, i)    — read text column at zero-based index
#   Db.column_count(stmt)      — number of columns in the prepared row
#   Db.column_name(stmt, i)    — name of column at zero-based index
#   Db.finalize(stmt)          — release the prepared stmt
#   Db.bind_int(stmt, i, v)    — bind int to `?` param i (1-based)
#   Db.bind_text(stmt, i, v)   — bind string to `?` param i (1-based)
#   Db.bind_bool(stmt, i, v)   — bind 0/1 to `?` param i (1-based)
#   Db.last_insert_rowid       — id of the last INSERTed row
#   Db.changes                 — affected-row count of the last statement
#   Db.escape_string(s)        — SQL-quote a string value
#   Db.escape_int(n)           — render an integer for SQL inlining
#
# Stmt handles are opaque pointers (`:ptr`) returned by sqlite3_prepare_v2
# via the SQL.stmt_out out-buffer. Two value paths coexist: the lowerer
# INLINES literals (`escape_string`/`escape_int`, both shimmed below) and
# BINDS runtime values (`bind_*`) when the placeholder-bind gate is on
# (roundhouse#12, ROUNDHOUSE_PARAM_BINDS). Binding keys the
# prepared-statement cache by static shape (`WHERE id = ?`) instead of
# per-value, so a query hammered with varying ids reuses one cached stmt.
# `bind_text` is unblocked at the FFI layer (spinel #576 +
# matz/spinel#686 doc fix).
#
# Module-level state is `@pools`, N independent DbPool shards of M opaque
# dbh ptrs each (see `configure` for why it is sharded rather than one
# opaque dbh ptrs). `Db.current_conn` reads Thread.current[:db_conn] when
# set (request-scoped checkout) and falls back to the pool's first free
# handle otherwise (single-fiber test/dev mode). Existing call sites don't
# care which path they're on; they just call Db.X.
#
# Pattern mirrors `examples/ffi/sqlite/blog.rb` in the spinel repo —
# the same `ffi_func` declarations, the same out-buffer plumbing.

# Bare-metal SQLite3 FFI bindings. Only the surface area Roundhouse's
# lowerer-emitted `_adapter_*` methods need.
module SQL
  ffi_lib "sqlite3"

  ffi_const :OK,   0
  ffi_const :ROW,  100
  ffi_const :DONE, 101
  # sqlite3_column_type storage classes. NULL is the one the nullable
  # reads have always needed; the rest let `column_value` hand back the
  # driver's native type instead of everything-as-text.
  ffi_const :INTEGER_TYPE, 1
  ffi_const :FLOAT_TYPE, 2
  ffi_const :TEXT_TYPE, 3
  ffi_const :BLOB_TYPE, 4
  ffi_const :NULL_TYPE, 5
  # open_v2 flags: READWRITE | CREATE | URI. The first two are what plain
  # `sqlite3_open` uses, so a non-URI path opens identically either way.
  ffi_const :OPEN_URI_RWC, 70

  ffi_func :sqlite3_open,              [:str, :ptr],                          :int
  # `sqlite3_open` honors a `file:` URI only on a build compiled with
  # SQLITE_USE_URI; Ubuntu's is, but that is a property of the distro's sqlite
  # and not of this program. `open_v2` takes the flag explicitly, so a URI path
  # means the same thing on every build — and getting it wrong is quiet in the
  # worst way: without URI handling sqlite treats the whole
  # `file:x?mode=memory&cache=shared` string as a FILENAME and cheerfully
  # creates a file with that name, so the process serves a fresh empty database
  # from disk while every log line says it opened the one that was asked for.
  ffi_func :sqlite3_open_v2,           [:str, :ptr, :int, :str],              :int
  ffi_func :sqlite3_close,             [:ptr],                                :int
  # Online backup — copies one open database over another at the PAGE level.
  # Used to seed an in-memory database from a file: it replaces the whole
  # destination, so schema DDL already run against the empty destination is
  # simply superseded rather than conflicting. `step(-1)` means "all remaining
  # pages in one call", which is what a boot-time seed wants (the incremental
  # form exists to avoid holding a lock, and at boot nothing else is reading).
  ffi_func :sqlite3_backup_init,       [:ptr, :str, :ptr, :str],              :ptr
  ffi_func :sqlite3_backup_step,       [:ptr, :int],                          :int
  ffi_func :sqlite3_backup_finish,     [:ptr],                                :int
  ffi_func :sqlite3_exec,              [:ptr, :str, :ptr, :ptr, :ptr],        :int
  ffi_func :sqlite3_prepare_v2,        [:ptr, :str, :int, :ptr, :ptr],        :int
  ffi_func :sqlite3_step,              [:ptr],                                :int
  ffi_func :sqlite3_finalize,          [:ptr],                                :int
  # Prepared-statement reuse (roundhouse#12, Path A.1). `reset` rewinds a
  # stepped stmt so it can be re-stepped; `clear_bindings` drops any bound
  # params. Cached `Db.finalize` calls these instead of `sqlite3_finalize`,
  # which now runs only at pool shutdown (see DbConn#finalize_all).
  ffi_func :sqlite3_reset,             [:ptr],                                :int
  ffi_func :sqlite3_clear_bindings,    [:ptr],                                :int
  # Placeholder binding (roundhouse#12, Path A.2). `bind_int64` binds a
  # `?` param to an integer (int64 so lobsters-scale ids don't truncate);
  # `bind_text` binds a string — the trailing two args are `nbyte` and
  # the destructor, both passed as `-1`: nbyte -1 lets sqlite strlen the
  # NUL-terminated text, and destructor -1 is `SQLITE_TRANSIENT`
  # (`((sqlite3_destructor_type)-1)`) so sqlite copies the bytes before
  # returning — safe even if the Ruby string is later GC'd. int→:ptr
  # coercion for the -1 destructor is the spinel primitive validated in
  # spinel's test/ffi_ptr_int_literal.rb.
  ffi_func :sqlite3_bind_int64,        [:ptr, :int, :long],                   :int
  ffi_func :sqlite3_bind_text,         [:ptr, :int, :str, :int, :ptr],        :int
  # `sqlite3_column_type` reports the storage class of a column in the
  # current row; 5 is SQLITE_NULL. The nullable-column reads below need
  # it because `sqlite3_column_int` cannot distinguish a stored 0 from
  # NULL (both come back as 0).
  ffi_func :sqlite3_column_type,       [:ptr, :int],                          :int
  ffi_func :sqlite3_column_int,        [:ptr, :int],                          :int
  ffi_func :sqlite3_column_double,     [:ptr, :int],                          :double
  ffi_func :sqlite3_column_text,       [:ptr, :int],                          :str
  ffi_func :sqlite3_column_count,      [:ptr],                                :int
  ffi_func :sqlite3_column_name,       [:ptr, :int],                          :str
  ffi_func :sqlite3_errmsg,            [:ptr],                                :str
  ffi_func :sqlite3_last_insert_rowid, [:ptr],                                :long
  ffi_func :sqlite3_changes,           [:ptr],                                :int

  # Out-params — sqlite3_open writes the db handle here, prepare_v2
  # writes the stmt handle. 8 bytes is enough for a 64-bit pointer.
  ffi_buffer :db_out,   8
  ffi_buffer :stmt_out, 8
  # Separate from db_out: the seed source is opened while the pool's handles
  # already exist, and reusing db_out would overwrite a live one.
  ffi_buffer :seed_out, 8
  ffi_read_ptr :read_ptr, 0
end

# One pooled SQLite connection plus its prepared-statement cache
# (roundhouse#12). Prepared stmts are bound to the connection they were
# prepared on and carry their own cursor, so the cache MUST be
# per-connection: a global SQL->stmt map would let two cooperative fibers
# (each leasing a different pool handle) hand back the same stmt and
# corrupt each other's cursor mid-iteration. Because the pool leases a
# DbConn to exactly one fiber at a time, the per-connection cache is also
# concurrency-safe without a mutex.
#
# Cache key is the fully-composed SQL string. With the placeholder-bind
# gate on (roundhouse#12), runtime values emit as `?` and the key is the
# static query shape (`WHERE id = ?`), so a query hammered with varying
# ids reuses one entry. With the gate off the lowerer inlines literals
# (`WHERE id = 1`) and id-bearing queries key per-id; the CAP below
# bounds growth in that mode.
#
# CAP bounds the CACHE. Until 2026-07-29 it did NOT bound the prepared
# STATEMENTS, and the difference was the AOT lane's whole memory story.
# Past the cap `prepare_cached` still prepared a stmt and returned it
# UNCACHED, and `Db.finalize` — which resets rather than closes, correct
# for a cached stmt that must survive for its next use — never finalized
# it. Nothing held a reference, so every query past the cap leaked one
# sqlite3 stmt and its compiled program. With the bind gate OFF (the
# default) keys are per-VALUE, so the cap is reached almost immediately
# and effectively every query leaked: measured at ~8 MB per 114-visit
# benchmark iteration, growing linearly to the 535 MB peak the published
# page reports — the worst cell on it, and above Rails.
#
# What made that hard to see from outside: it is invisible to the obvious
# probes. Replaying ONE route is flat however many times (its few shapes
# fit under the cap), and it is indifferent to page size — 4560 renders of
# the 292 KB /u tree retain the same ~27 MB as 4560 renders of the 2 KB
# /about. Only a MIX of routes overflows the cap, so the growth looked
# like it tracked route variety rather than the cap it actually tracked.
#
# The fix is the CRuby shim's (db_cruby.rb): never hand back an uncached
# stmt — bound the cache by EVICTING, and finalize what's evicted. The
# one hazard is closing a stmt some open cursor still holds; lobsters'
# comment tree nests cursors, and an outer one can stay open across many
# inner queries. CRuby can check directly (it hands out an index into a
# @rows table and can ask whether an entry still holds a stmt); this shim
# hands back the raw ptr, so it has no such table to consult.
#
# So eviction runs at the REQUEST BOUNDARY instead, where no cursor is
# open by construction — `with_connection` returns only after the whole
# request body has run. Within a request the cache is a soft bound; at
# every boundary it comes back down to CAP and the excess is finalized.
# Bounded, and it can never close a live stmt.
# One cache entry: the composed SQL and its prepared stmt ptr. A concrete
# user class (not a raw ptr) so an Array of these types concretely the way
# ConnectionPool's @free does — spinel infers the element type from the
# first `push`, which lets `length`/`[]` resolve (a poly_array of bare ptrs
# does not support them).
class Stmt
  def initialize(sql, ptr)
    @sql = sql
    @ptr = ptr
  end

  def sql
    @sql
  end

  def ptr
    @ptr
  end
end

# One cached SELECT result in a request's query cache: the rows as they
# were stepped, every column of each, plus whether the consumer reached
# the end. Cells are stored by STORAGE CLASS in four parallel typed
# arrays (kind + int + float + text, indexed row * ncols + col) rather
# than as one poly row array, so a replayed read is a typed array index
# and nothing is boxed. `eof` false means the first consumer stopped
# early (a `LIMIT 1` reader steps once and never asks again); a replay
# that wants more than the recorded prefix promotes to a real
# re-execution (see DbConn#qc_step?), the same rule as db_cruby.rb.
class QcEntry
  def initialize(sql, ncols, names)
    @sql = sql
    @ncols = ncols
    @names = names
    @kinds = []
    @ints = []
    @floats = [0.0]
    @floats.pop
    @texts = [""]
    @texts.pop
    @nrows = 0
    @eof = false
  end

  def qc_sql
    @sql
  end

  def ncols
    @ncols
  end

  def col_names
    @names
  end

  def nrows
    @nrows
  end

  def eof
    @eof
  end

  def mark_eof
    @eof = true
    nil
  end

  # Copy the current row of a real stmt, every column, by storage class:
  # 0 NULL, 1 INTEGER, 2 REAL, 3 TEXT (BLOB reads as text, as
  # Db.column_value does).
  def record_row(stmt)
    i = 0
    while i < @ncols
      t = SQL.sqlite3_column_type(stmt, i)
      if t == SQL::NULL_TYPE
        @kinds.push(0)
        @ints.push(0)
        @floats.push(0.0)
        @texts.push("")
      elsif t == SQL::INTEGER_TYPE
        @kinds.push(1)
        @ints.push(SQL.sqlite3_column_int(stmt, i))
        @floats.push(0.0)
        @texts.push("")
      elsif t == SQL::FLOAT_TYPE
        @kinds.push(2)
        @ints.push(0)
        @floats.push(SQL.sqlite3_column_double(stmt, i))
        @texts.push("")
      else
        @kinds.push(3)
        @ints.push(0)
        @floats.push(0.0)
        v = SQL.sqlite3_column_text(stmt, i)
        @texts.push(v.nil? ? "" : v + "")
      end
      i += 1
    end
    @nrows += 1
    nil
  end

  def cell_kind(row, i)
    @kinds[row * @ncols + i]
  end

  def cell_int(row, i)
    @ints[row * @ncols + i]
  end

  def cell_float(row, i)
    @floats[row * @ncols + i]
  end

  def cell_text(row, i)
    @texts[row * @ncols + i]
  end
end

# A replay in progress: which entry, which row the cursor sits on (-1
# before the first step), and the real stmt it promoted to when the
# recorded prefix ran out (0 = still replaying).
class QcCursor
  def initialize(entry)
    @entry = entry
    @pos = -1
    @promoted = false
    @real_ptr = nil
  end

  def promoted
    @promoted
  end

  def qc_entry
    @entry
  end

  def row_pos
    @pos
  end

  def advance_row
    @pos += 1
    nil
  end

  def real_ptr
    @real_ptr
  end

  def promote_to(ptr)
    @promoted = true
    @real_ptr = ptr
    nil
  end
end

class DbConn
  CAP = 128

  def initialize(dbh)
    @dbh = dbh
    @entries = []
    # The per-request query cache (see `qc_begin`). Off until a request
    # leases this connection, so scripts and tests that call Db directly
    # see plain round trips unless they ask.
    @qc_on = false
    @qc_by_sql = {}
    @qc_recording = {}
    @qc_cursors = []
  end

  def dbh
    @dbh
  end

  # Return a cached prepared stmt for `sql`, preparing+caching on miss.
  # Linear scan over a ptr-keyed structure spinel would not type; the hit
  # rate is what matters here, not the lookup's constant.
  #
  # EVERY prepared stmt goes into @entries, with no cap check. That is
  # what makes `Db.finalize`'s reset-don't-close correct for all of them:
  # a stmt this method hands back is always reachable for reuse, and
  # always finalizable at `trim!`. Capping HERE is what leaked.
  def prepare_cached(sql)
    i = 0
    while i < @entries.length
      e = @entries[i]
      return e.ptr if e.sql == sql
      i += 1
    end
    # `SQL.stmt_out` is ONE 8-byte out-buffer for the whole process (an
    # `ffi_buffer`, static C storage). Under parallel OS workers two
    # connections preparing at the same moment wrote their statement
    # pointers into it in turn, and one of them read the other's: a
    # statement then belonged to two connections, and the second use of
    # it — a `sqlite3_clear_bindings` on a handle another thread was
    # stepping, or a finalize of a statement already finalized — took the
    # process down within a second of parallel load on the room page
    # (matz/spinel#4312: needed many requests on distinct connections,
    # not the collector; the lease and the pool were clean). The prepare
    # and the read of its out-buffer are one critical section now. No
    # raise inside the lock: `synchronize` has no ensure on this lane.
    rc = 0
    st = 0
    Db.prepare_lock.synchronize do
      rc = SQL.sqlite3_prepare_v2(@dbh, sql, -1, SQL.stmt_out, nil)
      st = rc == SQL::OK ? SQL.read_ptr(SQL.stmt_out) : 0
    end
    if rc != SQL::OK
      raise "Db.prepare failed (" + rc.to_s + "): " + SQL.sqlite3_errmsg(@dbh) + " — sql: " + sql
    end
    @entries.push(Stmt.new(sql, st))
    st
  end

  # Bring the cache back to CAP, finalizing what's dropped. Called at the
  # request boundary (`Db.with_connection`), never mid-request: a stmt an
  # open cursor still holds must not be closed, and between requests there
  # are none.
  #
  # Keeps the CAP most-recent entries — the tail, since `prepare_cached`
  # appends. With per-value keys (bind gate off) recency is the best
  # available proxy for reuse; with shapes the whole set fits and this
  # never fires. Rebuilds the array rather than deleting in place: a fresh
  # Array whose first push is a Stmt types concretely, which is the same
  # reason `Stmt` is a class and not a bare ptr.
  def trim!
    return nil if @entries.length <= CAP
    keep = []
    drop_before = @entries.length - CAP
    i = 0
    while i < @entries.length
      if i < drop_before
        SQL.sqlite3_finalize(@entries[i].ptr)
      else
        keep.push(@entries[i])
      end
      i += 1
    end
    @entries = keep
    nil
  end

  # ── the per-request query cache ─────────────────────────────────────
  #
  # Rails wraps every request in the Active Record query cache: an
  # identical SELECT within one request replays the first result, and
  # any write empties the cache. The CRuby shim (db_cruby.rb) has had
  # the same discipline since issue #12; this lane had only the
  # prepared-statement cache above, so every repeat was a round trip —
  # campfire's `Current.account` is `Account.first`, asked 23 times per
  # page, and a message's `room` 40 times on a room page. Rails answers
  # those from this cache, which is why its count is what it is.
  #
  # The cache lives on the CONNECTION, which `Db.with_connection` leases
  # to exactly one request at a time — so it needs no fiber-local, and
  # a thread-per-connection server leases the same way.
  #
  # Handles: a replay is an INTEGER, the 1-based index of its cursor in
  # `@qc_cursors`; a real stmt is its FFI POINTER, as before. The two are
  # different kinds of value, and the `Db.step?` / `Db.column_*` /
  # `Db.finalize` funnel asks `is_a?(Integer)` to tell them apart, so the
  # emitted readers are unchanged. (Not a sign bit: a pointer cannot be
  # compared with 0.)
  #
  # Only non-parameterized SQL participates: a `?`-bearing string's
  # result depends on binds set after prepare, which are not in the key.
  def qc_begin
    @qc_on = true
    @qc_by_sql = {}
    @qc_recording = {}
    @qc_cursors = []
    nil
  end

  def qc_end
    @qc_on = false
    @qc_by_sql = {}
    @qc_recording = {}
    @qc_cursors = []
    nil
  end

  # A write: Rails empties the whole cache. In-flight captures are
  # abandoned too, so a SELECT that began before the write cannot
  # install pre-write rows after it.
  def qc_clear
    @qc_by_sql = {}
    @qc_recording = {}
    nil
  end

  # The handle for `sql`: a replay handle on a hit, else the real stmt
  # (recording its rows as they are stepped, when the cache is on).
  # Returns 0 as "no replay" so `Db.prepare` can tell the two apart.
  def qc_lookup(sql)
    return 0 if !@qc_on || sql.include?("?")
    e = @qc_by_sql[sql]
    return 0 if e.nil?
    @qc_cursors.push(QcCursor.new(e))
    @qc_cursors.length
  end

  # Start recording a real stmt's rows for `sql`. A stmt already being
  # recorded (the same SQL re-prepared while its first cursor is still
  # open hands back the same pointer) is left alone.
  def qc_record(sql, ptr)
    return nil if !@qc_on || sql.include?("?")
    return nil if !@qc_recording[ptr].nil?
    ncols = SQL.sqlite3_column_count(ptr)
    names = []
    i = 0
    while i < ncols
      n = SQL.sqlite3_column_name(ptr, i)
      names.push(n.nil? ? "" : n + "")
      i += 1
    end
    @qc_recording[ptr] = QcEntry.new(sql, ncols, names)
    nil
  end

  # Called after every real step. Appends the row (or marks eof).
  def qc_record_step(ptr, has_row)
    return nil if !@qc_on
    e = @qc_recording[ptr]
    return nil if e.nil?
    if has_row
      e.record_row(ptr)
    else
      e.mark_eof
    end
    nil
  end

  # Called at a real stmt's finalize: publish its capture, even a
  # partial one — the next identical SELECT replays the consumed prefix
  # and promotes past it only if it wants more.
  def qc_install(ptr)
    return nil if !@qc_on
    e = @qc_recording[ptr]
    return nil if e.nil?
    @qc_recording.delete(ptr)
    @qc_by_sql[e.qc_sql] = e if @qc_by_sql[e.qc_sql].nil?
    nil
  end

  def qc_cursor(handle)
    @qc_cursors[handle - 1]
  end

  # The real stmt a replay promoted to, or nil while it is still replaying.
  def qc_ptr(handle)
    c = qc_cursor(handle)
    return nil if !c.promoted
    c.real_ptr
  end

  def qc_step?(handle)
    c = qc_cursor(handle)
    return SQL.sqlite3_step(c.real_ptr) == SQL::ROW if c.promoted
    e = c.qc_entry
    if c.row_pos + 1 < e.nrows
      c.advance_row
      return true
    end
    return false if e.eof
    # The first consumer stopped before the end and this one wants more:
    # re-run the real statement and fast-forward past what was replayed.
    ptr = prepare_cached(e.qc_sql)
    n = 0
    while n < e.nrows
      SQL.sqlite3_step(ptr)
      n += 1
    end
    c.promote_to(ptr)
    SQL.sqlite3_step(ptr) == SQL::ROW
  end

  def qc_finalize(handle)
    c = qc_cursor(handle)
    if c.promoted
      SQL.sqlite3_reset(c.real_ptr)
      SQL.sqlite3_clear_bindings(c.real_ptr)
    end
    nil
  end

  # Cell reads for a replaying cursor, by the recorded storage class.
  def qc_kind(handle, i)
    c = qc_cursor(handle)
    c.qc_entry.cell_kind(c.row_pos, i)
  end

  def qc_int(handle, i)
    c = qc_cursor(handle)
    k = c.qc_entry.cell_kind(c.row_pos, i)
    return c.qc_entry.cell_int(c.row_pos, i) if k == 1
    return c.qc_entry.cell_float(c.row_pos, i).to_i if k == 2
    return c.qc_entry.cell_text(c.row_pos, i).to_i if k == 3
    0
  end

  def qc_float(handle, i)
    c = qc_cursor(handle)
    k = c.qc_entry.cell_kind(c.row_pos, i)
    return c.qc_entry.cell_float(c.row_pos, i) if k == 2
    return c.qc_entry.cell_int(c.row_pos, i).to_f if k == 1
    return c.qc_entry.cell_text(c.row_pos, i).to_f if k == 3
    0.0
  end

  def qc_text(handle, i)
    c = qc_cursor(handle)
    k = c.qc_entry.cell_kind(c.row_pos, i)
    return c.qc_entry.cell_text(c.row_pos, i) if k == 3
    return c.qc_entry.cell_int(c.row_pos, i).to_s if k == 1
    return c.qc_entry.cell_float(c.row_pos, i).to_s if k == 2
    ""
  end

  def qc_column_count(handle)
    qc_cursor(handle).qc_entry.ncols
  end

  def qc_column_name(handle, i)
    qc_cursor(handle).qc_entry.col_names[i]
  end

  # Real finalize of every cached stmt — pool-shutdown path only.
  def finalize_all
    i = 0
    while i < @entries.length
      SQL.sqlite3_finalize(@entries[i].ptr)
      i += 1
    end
  end
end

# Dedicated SQLite connection pool (roundhouse#12). A single-use object so
# its instance ivars type concretely: @conns is a DbConn PtrArray (objects
# keep their tag, unlike the generic ConnectionPool's int slot), @free is
# an IntArray stack of available indices into @conns.
class DbPool
  # PRAGMAs ARE PER-CONNECTION, so they run on every handle the pool opens,
  # not once on the first. (The same reason runtime/go/v2/db.go sets them in
  # the DSN rather than with a one-shot Exec.)
  #
  # WHY THESE, MEASURED. This runtime was the only one setting no pragmas at
  # all — kotlin, csharp, go and typescript each set theirs — and a file-backed
  # server pays for that on every request. SQLite's default page cache is 2 MB;
  # the lobsters fixture is 42 MB, so a long-lived process re-reads the same
  # pages from the OS on every visit instead of keeping the working set. On the
  # bench that showed up as ~400 `pread64` syscalls per /top visit, with 75% of
  # the profile inside libsqlite3 and under 6% in compiled code. One query
  # repeated six times on one connection against that fixture: 1,762 preads at
  # the default, 516 with a large cache — the rest was re-reading what it had
  # already read.
  #
  # cache_size is NEGATIVE on purpose: sqlite reads a positive value as a page
  # COUNT and a negative one as a KiB budget, so -65536 is 64 MiB regardless of
  # page_size, while 65536 would be 64k PAGES — 256 MiB at the 4 KiB default,
  # and silently different again on a fixture built with another page size.
  #
  # An in-memory database ignores all three (its pages are already the heap),
  # which is harmless: this is a fixed cost at boot on a path that then does
  # nothing.
  PRAGMAS = [
    "PRAGMA cache_size=-65536",
    # Read pages straight out of the page cache by mapping the file, which
    # drops the read() syscall and its copy for everything that fits.
    "PRAGMA mmap_size=268435456",
    # WAL for readers-alongside-writer, and a bounded wait rather than an
    # immediate SQLITE_BUSY when a write does overlap — what the sibling
    # runtimes set, and what a pool of connections on one file needs.
    "PRAGMA journal_mode=WAL",
    "PRAGMA busy_timeout=5000",
  ].freeze

  def initialize(path, n)
    @conns = []
    @free  = []
    @lock  = Mutex.new
    @cv    = ConditionVariable.new
    i = 0
    while i < n
      rc = SQL.sqlite3_open_v2(path, SQL.db_out, SQL::OPEN_URI_RWC, nil)
      if rc != SQL::OK
        # Best-effort error surface — sqlite3_errmsg requires a valid db
        # handle, which we don't have on open failure. The numeric rc +
        # path are the only signals we can raise pre-handle.
        raise "Db.configure: sqlite3_open(" + path + ") failed (" + rc.to_s + ")"
      end
      dbh = SQL.read_ptr(SQL.db_out)
      # Deliberately not raising on a refused pragma. Every one of these is an
      # optimization or a politeness; none changes a query's RESULT. A build of
      # sqlite that declines one (journal_mode=WAL on a read-only mount, say)
      # should still serve, slower, rather than fail to boot — and a pragma
      # that silently did nothing is what the numbers above would reveal.
      PRAGMAS.each { |p| SQL.sqlite3_exec(dbh, p, nil, nil, nil) }
      @conns.push(DbConn.new(dbh))
      @free.push(i)
      i += 1
    end
  end

  def available
    n = 0
    @lock.synchronize do
      n = @free.length
    end
    n
  end

  # Pop a free connection index (LIFO), parking this green thread while
  # the pool is empty: `release` signals. (The lease used to spin on a
  # scheduler pause; a condition variable is the shape for threads.)
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

  # Finalize every cached stmt on every connection, then close the handles.
  def close_all
    i = 0
    while i < @conns.length
      c = @conns[i]
      c.finalize_all
      SQL.sqlite3_close(c.dbh)
      i += 1
    end
  end
end

# Temporal intrinsics (`ActiveSupport.parse_db_time` in the synthesized
# column readers, `db_now` in fill_timestamps) — chained off Db, the one
# require every persistence-touching bootstrap (main.rb AND the emitted
# test_helper) already loads. Mirrors the db_cruby/db_jruby chain the
# CRuby/JRuby materialization inserts; before this file existed the
# calls were unresolved and spinel's old silent gate nil'd them
# (spinel#1661 — the strict gate in spinel 1356cb14 surfaced it).
require_relative "active_support_time_parsing"

module Db
  # Own connection pool (roundhouse#12). Was
  # ActiveRecord::ConnectionAdapters::ConnectionPool, but that generic
  # stores handles in an sp_IntArray slot — a DbConn* flattens to a bare
  # machine word there and reads back tagged INT, so a later `.dbh` call
  # (guarded `tag == OBJ`) silently no-ops to NULL. A dedicated single-use
  # pool object (DbPool, below) keeps its connections in an INSTANCE-ivar
  # array, which spinel types as a concrete DbConn PtrArray (same shape as
  # DbConn#@entries) — preserving the object tag.
  @pools = nil
  # Query-log capture (issue #27). `nil` ⇒ not capturing; an Array ⇒
  # accumulate the SQL each prepare/exec issues. Kept in parity with the
  # cruby shim (db_cruby.rb); see `capture_sql` below.
  @query_log = nil
  # RH_SQL_TRACE=1 prints every SQL string this process prepares to
  # stderr, the way `RAILS_LOG_LEVEL=debug` shows Rails' queries: a real
  # sqlite3 round trip as `  SQL ...`, a replay from the request's query
  # cache (DbConn#qc_*) as `  CACHE ...`, which is Rails' own spelling.
  # scripts/campfire-queries counts the two apart.
  @sql_trace = false

  # Pool size: kwarg wins; otherwise DATABASE_POOL_SIZE env (the same
  # knob the rust target reads — set it to the server's max concurrent
  # connections so no fiber parks waiting for a handle); else a modest
  # default. Each entry is one FFI sqlite3 handle to `path`.
  def self.configure(path, pool_size: 8)
    @sql_trace = ENV.fetch("RH_SQL_TRACE", "") != ""
    n = pool_size
    ev = ENV["DATABASE_POOL_SIZE"]
    if !ev.nil? && ev != ""
      n = ev.to_i
    end
    # SHARDED, because one pool means one Mutex on every request and that
    # mutex — not the queries under it — was what pinned the server to a
    # couple of OS workers. `with_connection` takes the pool lock twice per
    # request (lease, release) and holds it for a few instructions; measured
    # on campfire's cheapest authenticated route at 12 workers, that cost
    # 2,449 req/s on 1.32 cores. Taking the same lock once per THREAD instead
    # of once per request gave 15,648 req/s on 8.05 cores, evenly spread over
    # all twelve — 6x, from deleting contention rather than work.
    #
    # A thread cannot simply KEEP a connection, which is what that experiment
    # did: campfire holds a green thread per WebSocket, thousands of them, and
    # pinning a handle to each would exhaust any pool. So the pool is split
    # into independent shards and a thread is assigned one for its life. The
    # shard is a whole DbPool — the same class, unchanged — so this adds no
    # new locking of its own: a thread leases from and releases to one shard,
    # blocks only on that shard's own condition variable, and is woken only by
    # its own shard's releases. There is no cross-shard wakeup to lose.
    #
    # The cost of the split is that a shard can be busy while another is idle.
    # Threads are assigned round-robin, so shards carry equal numbers of them;
    # sizing keeps at least 4 handles per shard, since a shard of one is a
    # mutex with extra steps.
    stripes = n / 4
    stripes = 1 if stripes < 1
    stripes = 8 if stripes > 8
    @pools = []
    per = n / stripes
    per = 1 if per < 1
    i = 0
    while i < stripes
      @pools.push(DbPool.new(path, per))
      i += 1
    end
    @assign_lock = Mutex.new
    @next_pool = 0
  end

  # The shard this thread uses, chosen once and remembered. Round-robin
  # under a lock taken ONCE per thread — the thing being avoided is a lock
  # per request, not a lock ever.
  def self.pool_for_thread
    pi = Thread.current[:db_pool]
    return @pools[pi] if pi != nil
    n = 0
    @assign_lock.synchronize do
      n = @next_pool
      @next_pool = n + 1
    end
    pi = n % @pools.length
    Thread.current[:db_pool] = pi
    @pools[pi]
  end

  # Replace this database's contents with those of the file at `src_path`,
  # page for page, then leave planner statistics behind.
  #
  # WHAT IT IS FOR. An in-memory database is the shape the ruby-bench lobsters
  # benchmark measures, and the shape every interpreted lane here already runs
  # (`file:lobsters_bench?mode=memory&cache=shared`, seeded by
  # scripts/lobsters-replay through the sqlite3 gem's Backup). This is the same
  # seed for a compiled binary, which cannot borrow the harness's gem: without
  # it the AOT lane serves from disk while the lanes it is charted against serve
  # from RAM, and pays ~400 `pread64` syscalls per query-heavy request that they
  # do not — a difference read off the profile as the compiler being slower.
  #
  # A page-level backup, not table-by-table SQL: it copies indexes and
  # sqlite_stat1 as they are, needs no list of tables, and cannot get insertion
  # order wrong against foreign keys. It REPLACES the destination, so schema DDL
  # already run against the empty database is superseded rather than conflicting
  # — the same reason the CRuby lane can seed after boot.
  #
  # Runs on the pool's first connection. With a shared-cache in-memory database
  # every pooled handle sees one database, so seeding through one seeds all;
  # with a file database it is the same file. Boot-time only — a live reader
  # would hold a lock the backup has to wait for.
  def self.seed_from_file(src_path)
    dest = current_conn.dbh
    rc = SQL.sqlite3_open_v2(src_path, SQL.seed_out, SQL::OPEN_URI_RWC, nil)
    if rc != SQL::OK
      raise "Db.seed_from_file: cannot open " + src_path + " (" + rc.to_s + ")"
    end
    src = SQL.read_ptr(SQL.seed_out)
    bk = SQL.sqlite3_backup_init(dest, "main", src, "main")
    if bk.nil?
      # errmsg lives on the DESTINATION handle for a failed backup_init.
      msg = SQL.sqlite3_errmsg(dest)
      SQL.sqlite3_close(src)
      raise "Db.seed_from_file: backup_init failed: " + msg
    end
    step_rc = SQL.sqlite3_backup_step(bk, -1)
    SQL.sqlite3_backup_finish(bk)
    SQL.sqlite3_close(src)
    # DONE, not OK, is success for a completed step(-1); OK means pages remain,
    # which for -1 would mean it did not finish.
    if step_rc != SQL::DONE
      raise "Db.seed_from_file: backup_step(" + src_path + ") -> " + step_rc.to_s
    end
    # The fixture ships without sqlite_stat1, and an unplanned database
    # misplans the hottest-stories SELECT into a full-table walk plus a
    # temp-btree sort instead of reading hotness_idx to the LIMIT. Every other
    # lane ANALYZEs its seeded copy for exactly this reason; skipping it here
    # would hand one lane a planner accident the others do not have.
    exec("ANALYZE")
  end

  # The DbConn this fiber should read/write through. Set by
  # `with_connection` (request scope); falls back to the first connection
  # for single-fiber test/dev/boot (e.g. DDL) modes. `Fiber[:k]` is
  # spinel's per-fiber storage indexer (#577/#578). The stored value is a
  # real DbConn object (tag OBJ), so the `.dbh`/`.prepare_cached` calls on
  # the result resolve — unlike the int-boxed ConnectionPool path.
  # Serialises `sqlite3_prepare_v2` and the read of its shared out-buffer
  # across OS workers (DbConn#prepare_cached). Created at load: the first
  # prepare can come from any thread.
  @prepare_lock = Mutex.new
  def self.prepare_lock
    @prepare_lock
  end

  def self.current_conn
    c = Thread.current[:db_conn]
    return c if !c.nil?
    @pools[0].first
  end

  # Request-scoped connection lease for the thread-per-connection
  # server. Leases a connection index (parking on the pool's condition
  # variable while none is free), binds its DbConn to this thread's
  # storage, runs the block, then releases the index. With pool_size >=
  # the workers' concurrency the wait never trips.
  #
  # NOTE: no begin/ensure (not used elsewhere in spinel-compiled code), so
  # a raise inside the block leaks the lease — acceptable on the happy
  # path; revisit if the dispatch path starts raising under load.
  def self.with_connection
    pool = pool_for_thread
    idx = pool.lease
    conn = pool.conn(idx)
    Thread.current[:db_conn] = conn
    # Rails wraps every request in the query cache; so does this lease.
    conn.qc_begin
    result = yield
    conn.qc_end
    # Every cursor this request opened is closed by now, so trimming the
    # stmt cache here can't close a live one. This is the only place the
    # cache is bounded — `prepare_cached` deliberately caps nothing, since
    # a stmt it refused to cache is a stmt nothing can ever finalize.
    conn.trim!
    Thread.current[:db_conn] = nil
    pool.release(idx)
    result
  end

  def self.close
    return if @pools.nil?
    i = 0
    while i < @pools.length
      @pools[i].close_all
      i += 1
    end
    @pools = nil
  end

  # DDL + INSERT/UPDATE/DELETE. `sqlite3_exec` doesn't return rows;
  # callers that want last_insert_rowid / changes consult those
  # accessors immediately after.
  # The query cache, by hand — for tests and scripts that call Db outside
  # a `with_connection` lease (the lease brackets it on its own).
  def self.query_cache_begin
    current_conn.qc_begin
  end

  def self.query_cache_end
    current_conn.qc_end
  end

  def self.exec(sql)
    record_query(sql)
    conn = current_conn
    # Any exec is (per the Db contract) DDL or a write — Rails
    # invalidates the whole query cache on write; so do we.
    conn.qc_clear
    h = conn.dbh
    rc = SQL.sqlite3_exec(h, sql, nil, nil, nil)
    if rc != SQL::OK
      msg = SQL.sqlite3_errmsg(h)
      raise ActiveRecord::RecordNotUnique, msg if Db.unique_violation?(msg)
      raise "Db.exec failed (" + rc.to_s + "): " + msg + " — sql: " + sql
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
    message.include?("UNIQUE constraint failed")
  end

  # Returns the stmt pointer; caller advances with `step?`, reads
  # columns with `column_int` / `column_text`, releases with
  # `finalize`. The -1 length argument lets sqlite measure the SQL
  # itself (NUL-terminated).
  def self.prepare(sql)
    conn = current_conn
    handle = conn.qc_lookup(sql)
    if handle != 0
      # A replay is not a round trip: the capture log (Rails' SQLCounter
      # ignores CACHE events too) and the trace both say so.
      if @sql_trace
        $stderr.puts "  CACHE " + sql
      end
      return handle
    end
    record_query(sql)
    ptr = conn.prepare_cached(sql)
    conn.qc_record(sql, ptr)
    ptr
  end

  # Query-log capture — see db_cruby.rb for the full rationale (the
  # test-side analog of Rails' `sql.active_record` SQLCounter; the one
  # instrument that can see the includes(:assoc) N+1 `compare` is blind
  # to, issue #27). Kept in parity across both Db shims.
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
    if @sql_trace
      $stderr.puts "  SQL " + sql
    end
  end

  def self.sql_trace?
    @sql_trace
  end

  def self.step?(stmt)
    if stmt.is_a?(Integer)
      return current_conn.qc_step?(stmt)
    end
    has_row = SQL.sqlite3_step(stmt) == SQL::ROW
    current_conn.qc_record_step(stmt, has_row)
    has_row
  end

  # A replay handle that has promoted to a real stmt reads through that
  # stmt; one still replaying reads its recorded cells. Returns the
  # pointer to read through, or nil for "answer from the cells".
  def self.replay_ptr(stmt)
    current_conn.qc_ptr(stmt)
  end

  def self.column_int(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      return current_conn.qc_int(stmt, i) if p.nil?
      stmt = p
    end
    SQL.sqlite3_column_int(stmt, i)
  end

  def self.column_float(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      return current_conn.qc_float(stmt, i) if p.nil?
      stmt = p
    end
    SQL.sqlite3_column_double(stmt, i)
  end

  # The libsqlite3 column buffer is invalidated by the next step or
  # finalize on the same stmt. Force a copy by appending an empty
  # string so the value survives downstream use. Mirrors the pattern
  # in spinel's reference blog.rb FFI example.
  def self.column_text(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      return current_conn.qc_text(stmt, i) if p.nil?
      stmt = p
    end
    s = SQL.sqlite3_column_text(stmt, i)
    if s.nil?
      ""
    else
      s + ""
    end
  end

  # Raw typed column read — the driver's native value, the same contract
  # the gem-backed shims (db_cruby/db_jruby) answer: Integer for an
  # INTEGER storage class, Float for REAL, String for TEXT, nil for NULL.
  #
  # Dispatches on the STORAGE CLASS of the value in THIS row, not on the
  # column's declared type. SQLite is dynamically typed and both gem
  # shims report what is actually stored, so storage class is what keeps
  # the AOT tree's hydration in step with the CRuby lane. Caching
  # `sqlite3_column_decltype` once per statement would save the probe
  # below, but it would diverge from the shims exactly when storage class
  # and declaration disagree — and it cannot detect NULL at all, which is
  # the whole reason the nullable reads need a per-row probe.
  #
  # Costs one `sqlite3_column_type` call per column per row on top of the
  # value read. That is the same price `column_int_opt` and friends
  # already pay, and it is unavoidable: `sqlite3_column_int` cannot tell
  # a stored 0 from NULL. Before this, every value arrived as text, so
  # `pluck(:thread_id)` handed back `"19948"` where the model attribute
  # held `19948` and every Hash lookup between them missed.
  #
  # Integers read through `sqlite3_column_int` (32-bit), matching
  # `column_int` and the `from_stmt` hydration path; a corpus with keys
  # past 2^31 would need `sqlite3_column_int64` wired for all of them
  # together, not just here. BLOB falls to the text read — no corpus
  # caller stores one, and text is a more useful stand-in than raising.
  def self.column_value(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      if p.nil?
        conn = current_conn
        k = conn.qc_kind(stmt, i)
        return nil if k == 0
        return conn.qc_int(stmt, i) if k == 1
        return conn.qc_float(stmt, i) if k == 2
        return conn.qc_text(stmt, i)
      end
      stmt = p
    end
    t = SQL.sqlite3_column_type(stmt, i)
    if t == SQL::NULL_TYPE
      nil
    elsif t == SQL::INTEGER_TYPE
      SQL.sqlite3_column_int(stmt, i)
    elsif t == SQL::FLOAT_TYPE
      SQL.sqlite3_column_double(stmt, i)
    else
      # Same copy-out as column_text — the libsqlite3 buffer is
      # invalidated by the next step or finalize on this stmt.
      s = SQL.sqlite3_column_text(stmt, i)
      if s.nil?
        nil
      else
        s + ""
      end
    end
  end

  # Nullable-column reads (see db_cruby.rb): NULL stays nil rather than
  # collapsing to "" / 0. Unlike the gem-backed shims there is no
  # native value to inspect, so these dispatch on the column's storage
  # class first.
  def self.column_int_opt(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      if p.nil?
        conn = current_conn
        return nil if conn.qc_kind(stmt, i) == 0
        return conn.qc_int(stmt, i)
      end
      stmt = p
    end
    if SQL.sqlite3_column_type(stmt, i) == SQL::NULL_TYPE
      nil
    else
      SQL.sqlite3_column_int(stmt, i)
    end
  end

  def self.column_float_opt(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      if p.nil?
        conn = current_conn
        return nil if conn.qc_kind(stmt, i) == 0
        return conn.qc_float(stmt, i)
      end
      stmt = p
    end
    if SQL.sqlite3_column_type(stmt, i) == SQL::NULL_TYPE
      nil
    else
      SQL.sqlite3_column_double(stmt, i)
    end
  end

  def self.column_text_opt(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      if p.nil?
        conn = current_conn
        return nil if conn.qc_kind(stmt, i) == 0
        return conn.qc_text(stmt, i)
      end
      stmt = p
    end
    s = SQL.sqlite3_column_text(stmt, i)
    if s.nil?
      nil
    else
      s + ""
    end
  end

  def self.column_bool_opt(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      if p.nil?
        conn = current_conn
        return nil if conn.qc_kind(stmt, i) == 0
        return conn.qc_int(stmt, i) != 0
      end
      stmt = p
    end
    if SQL.sqlite3_column_type(stmt, i) == SQL::NULL_TYPE
      nil
    else
      SQL.sqlite3_column_int(stmt, i) != 0
    end
  end

  def self.column_count(stmt)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      return current_conn.qc_column_count(stmt) if p.nil?
      stmt = p
    end
    SQL.sqlite3_column_count(stmt)
  end

  # libsqlite3's `sqlite3_column_name` returns a pointer owned by the
  # stmt; force a copy by appending an empty string so the value
  # survives the next step / finalize, mirroring `column_text`.
  def self.column_name(stmt, i)
    if stmt.is_a?(Integer)
      p = Db.replay_ptr(stmt)
      return current_conn.qc_column_name(stmt, i) if p.nil?
      stmt = p
    end
    s = SQL.sqlite3_column_name(stmt, i)
    if s.nil?
      ""
    else
      s + ""
    end
  end

  # roundhouse#12 Path A.1: with caching on, "finalize" means rewind the
  # cached stmt (reset cursor + clear any bound params) so the next call
  # reuses it. Real sqlite3_finalize runs only at pool close.
  def self.finalize(stmt)
    conn = current_conn
    if stmt.is_a?(Integer)
      conn.qc_finalize(stmt)
      return nil
    end
    conn.qc_install(stmt)
    SQL.sqlite3_reset(stmt)
    SQL.sqlite3_clear_bindings(stmt)
    nil
  end

  # Placeholder binding (roundhouse#12, Path A.2). Bind one `?` param
  # (1-based index, sqlite convention) on a prepared stmt before the
  # first `step?`. The lowerer emits `Db.bind_int`/`bind_text`/
  # `bind_bool` calls straight after `Db.prepare` when the query carries
  # runtime WHERE values, so those queries key the prepared-statement
  # cache by shape (`WHERE id = ?`) not value. A cached stmt was already
  # reset + clear_bindings'd at its previous `finalize`, so re-binding
  # here starts clean.
  def self.bind_int(stmt, idx, value)
    return nil if stmt.is_a?(Integer)
    SQL.sqlite3_bind_int64(stmt, idx, value)
  end

  def self.bind_text(stmt, idx, value)
    return nil if stmt.is_a?(Integer)
    SQL.sqlite3_bind_text(stmt, idx, value, -1, -1)
  end

  # SQLite has no native bool — bind 0/1, matching escape_bool's inline
  # form and the INTEGER affinity `t.boolean` columns get.
  def self.bind_bool(stmt, idx, value)
    return nil if stmt.is_a?(Integer)
    SQL.sqlite3_bind_int64(stmt, idx, value ? 1 : 0)
  end

  def self.last_insert_rowid
    SQL.sqlite3_last_insert_rowid(current_conn.dbh)
  end

  def self.changes
    SQL.sqlite3_changes(current_conn.dbh)
  end

  # Same SQL-value escaping shape as the gem-backed sibling. Single-
  # quote doubling matches sqlite's literal-string syntax; non-string
  # input goes through `to_s` first (Ruby semantics).
  def self.escape_string(s)
    "'" + s.to_s.gsub("'", "''") + "'"
  end

  def self.escape_int(n)
    n.to_i.to_s
  end

  # Nullable-column writes (see db_cruby.rb): nil renders NULL.
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
  # rows (an empty `IN ()` is a syntax error). Mirrors the cruby shim.
  def self.escape_int_list(ids)
    return "NULL" if ids.empty?

    ids.map { |i| i.to_i.to_s }.join(", ")
  end

  # SQLite stores booleans as 0/1 integers (no native bool type) —
  # mirrors the cruby sibling shim.
  def self.escape_bool(b)
    b ? "1" : "0"
  end

  # Read a boolean column. SQLite returns 0/1 (integer), widen to bool.
  def self.column_bool(stmt, idx)
    column_int(stmt, idx) != 0
  end
end
