//! Roundhouse Rust DB runtime.
//!
//! Hand-written helpers the Rust emitter copies verbatim into each
//! generated project as `src/db.rs`. Owns the per-test SQLite
//! connection and hides rusqlite borrowing from the generated code
//! — save/destroy/count/find all go through `with_conn`.
//!
//! Two entry points:
//!   - `setup_test_db(schema)` — thread-local `:memory:` connection
//!     for tests. Each test re-installs a fresh DB so prior-test
//!     state doesn't bleed across.
//!   - `open_production_db(path, schema)` — file-backed connections
//!     installed into a process-wide pool (`OnceLock<Vec<Mutex<
//!     Connection>>>`). Used by `main.rs` on server startup.
//!     Pool size = `DATABASE_POOL_SIZE` env var, defaulting to
//!     `std::thread::available_parallelism()`. SQLite is opened in
//!     WAL mode so N readers actually proceed in parallel.
//!     `with_conn` reaches either slot — test thread-local first,
//!     then the production pool (try_lock each entry; fall back to
//!     blocking-lock on slot 0).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use rusqlite::types::Value;
use rusqlite::Connection;

thread_local! {
    /// The connection the current thread's test (or request handler)
    /// uses. `None` until `setup_test_db` initializes it.
    static CONN: RefCell<Option<Connection>> = const { RefCell::new(None) };

    /// Test-only SQL capture log. `Some(vec)` while a `capture_sql`
    /// window is open on this thread; every `prepare`/`exec` records
    /// its SQL into it. Thread-local — like `CONN` above — so it's
    /// isolated across the parallel `#[tokio::test]` workers and
    /// scoped to the in-process request a test drives (axum-test's
    /// mock transport polls the handler inline on the test thread, so
    /// the queries land in this thread's log). The query counter is
    /// the only instrument that sees the `includes(:assoc)` N+1 that
    /// `compare` is structurally blind to (roundhouse#40, #27).
    static SQL_CAPTURE: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Process-wide sqlite connection pool for the production server.
/// `axum` handlers run on a multi-thread tokio runtime so per-thread
/// thread-locals don't work; each slot in this Vec is an independent
/// `Connection` guarded by its own `Mutex`, and `with_conn` picks
/// whichever slot it can `try_lock` first. SQLite is opened in WAL
/// mode (see `open_production_db`), which is what makes N readers
/// actually proceed in parallel. Pool size defaults to
/// `std::thread::available_parallelism()`; override with
/// `DATABASE_POOL_SIZE`.
static PROD_POOL: OnceLock<Vec<Mutex<Connection>>> = OnceLock::new();

/// rusqlite per-connection prepared-statement cache capacity
/// (roundhouse#12). Each `Connection` keeps an LRU of compiled statements
/// keyed by SQL; `prepare_cached` reuses them, skipping the re-parse the
/// blog's fixed query set otherwise paid on every request. Inlined
/// literals make id-bearing queries key per-id, so the LRU also bounds
/// memory — evicted statements are finalized by rusqlite, no leak.
/// Placeholder binding (the planned follow-on) makes the key the static
/// query shape. Default rusqlite capacity is 16; raise it to comfortably
/// hold the blog's working set.
const STMT_CACHE_CAP: usize = 128;

/// Initialize a fresh `:memory:` SQLite database on the current
/// thread, run the supplied schema DDL, and install the connection
/// so later `with_conn` calls can reach it. Replaces any connection
/// left over from a previous test that ran on the same thread.
///
/// `schema_sql` is the generated `crate::schema_sql::CREATE_TABLES`
/// string — passed explicitly so this file can stay target-agnostic
/// and compile standalone in the Roundhouse repo's runtime tree.
pub fn setup_test_db(schema_sql: &str) {
    let conn = Connection::open_in_memory().expect("open :memory: sqlite");
    conn.set_prepared_statement_cache_capacity(STMT_CACHE_CAP);
    conn.execute_batch(schema_sql).expect("run schema SQL");
    CONN.with(|c| *c.borrow_mut() = Some(conn));
}

/// Borrow the current thread's test connection, falling back to
/// the process-wide production connection. Panics if neither is
/// installed — callers are generated code that runs inside either
/// a test (whose setup already called `setup_test_db`) or a live
/// request (whose `main.rs` already called `open_production_db`).
pub fn with_conn<R, F: FnOnce(&Connection) -> R>(f: F) -> R {
    // Check the test-connection slot first — lets a production-
    // configured binary still run unit tests against a per-thread
    // in-memory DB. `CONN.with` runs its closure synchronously so
    // we can carry the closure-ownership handoff explicitly: if
    // the test slot has a connection, run `f` there and return
    // Ok(result); otherwise return Err(f) and run `f` against the
    // production mutex below.
    let result: Result<R, F> = CONN.with(|c| {
        let borrowed = c.borrow();
        match borrowed.as_ref() {
            Some(conn) => Ok(f(conn)),
            None => Err(f),
        }
    });
    match result {
        Ok(out) => out,
        Err(f) => {
            let pool = PROD_POOL.get().expect(
                "db not initialized; call setup_test_db or open_production_db first",
            );
            for slot in pool {
                if let Ok(guard) = slot.try_lock() {
                    return f(&guard);
                }
            }
            let guard = pool[0].lock().expect("prod DB mutex poisoned");
            f(&guard)
        }
    }
}

/// Open a file-backed sqlite database for the production server,
/// apply the schema DDL idempotently, and install it as the
/// process-wide connection. Creates intermediate directories if
/// needed — `better-sqlite3` creates the file but not its parent
/// dir, and rusqlite mirrors that behavior; the TS runtime hit
/// the same gotcha during smoke test, so we preempt it here.
///
/// `schema_sql` is the generated `schema_sql::CREATE_TABLES`
/// string. The emitter produces `CREATE TABLE IF NOT EXISTS`
/// directly, so re-opening an existing DB no-ops over the
/// already-present tables.
pub fn open_production_db(path: &str, schema_sql: &str) {
    let pool_size = std::env::var("DATABASE_POOL_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });
    open_production_pool(path, schema_sql, pool_size);
}

/// Pool builder shared by `open_production_db` and the unit tests.
/// Kept separate so tests can pick an explicit pool size without
/// reaching for `std::env::set_var` (which is `unsafe` under
/// Rust edition 2024).
pub fn open_production_pool(path: &str, schema_sql: &str, pool_size: usize) {
    // Turn off SQLite's global memory-status accounting before the library
    // initializes. With it on (the bundled-build default), every internal
    // malloc/free takes the process-wide `mem0` mutex, which serializes all
    // pool connections under load — profiled at ~65% of thread-time blocked
    // on that lock at c=64 (roundhouse#32). sqlite3_config is only effective
    // pre-initialization; if something already opened a connection it returns
    // SQLITE_MISUSE and accounting simply stays on (correct, just slower), so
    // the result is deliberately ignored.
    unsafe {
        rusqlite::ffi::sqlite3_config(rusqlite::ffi::SQLITE_CONFIG_MEMSTATUS, 0i32);
    }
    if path != ":memory:" {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent).expect("mkdir db parent");
            }
        }
    }
    let mut conns: Vec<Mutex<Connection>> = Vec::with_capacity(pool_size);
    for _ in 0..pool_size {
        let conn = Connection::open(path).expect("open sqlite db");
        conn.set_prepared_statement_cache_capacity(STMT_CACHE_CAP);
        conn.pragma_update(None, "journal_mode", "WAL")
            .expect("enable WAL");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign keys");
        // CREATE TABLE IF NOT EXISTS is idempotent on file-backed DBs;
        // applying per-conn is what lets a `:memory:` pool work, since
        // each `:memory:` is an independent database.
        conn.execute_batch(schema_sql).expect("apply schema");
        conns.push(Mutex::new(conn));
    }
    PROD_POOL
        .set(conns)
        .map_err(|_| "PROD_POOL already initialized")
        .expect("set PROD_POOL");
}

// ── Low-level prepare/step/column API ───────────────────────────────
//
// Per-statement state lives in a thread-local `STATEMENTS` table; the
// opaque `i64` stmt id indexes into it. Rows materialize on `prepare`
// — rusqlite's `Statement`/`Rows` borrow chain from `Connection` is
// awkward to thread through a `RefCell<HashMap<i64, _>>`, so we eat
// the up-front allocation in exchange for a self-contained per-stmt
// entry. Matches `runtime/crystal/db.cr`'s `Roundhouse::Db` API
// shape so the lowered model bodies (`Db.prepare(sql)`, `Db.step?
// (stmt)`, etc., from `src/lower/model_to_library/adapter_emit.rs`)
// emit the same calls under both targets.

/// A materialized prepared-statement entry: all rows fetched up front
/// + a cursor position + the most recently-stepped row snapshot.
struct StmtEntry {
    rows: Vec<Vec<Value>>,
    pos: usize,
    current: Option<Vec<Value>>,
}

thread_local! {
    static STATEMENTS: RefCell<HashMap<i64, StmtEntry>> =
        RefCell::new(HashMap::new());
    static NEXT_STMT_ID: Cell<i64> = const { Cell::new(0) };
    static LAST_INSERT_ROWID: Cell<i64> = const { Cell::new(0) };
}

/// `Db` namespace — the lowerer (`src/lower/model_to_library/
/// adapter_emit.rs` + `src/lower/arel/visitor.rs`) emits
/// `Db.prepare(sql)` / `Db.step?(stmt)` / `Db.column_int(stmt, i)`
/// against the synthesized per-model adapter methods. Mirrors the
/// Crystal target's `Roundhouse::Db` module member-for-member.
pub struct Db;

impl Db {
    /// Run a one-shot DDL/INSERT/UPDATE/DELETE. Captures
    /// `last_insert_rowid` so the subsequent accessor returns the
    /// freshly-inserted id (the typical `Db.exec(insert_sql);
    /// id = Db.last_insert_rowid` shape in lowered persistence).
    pub fn exec(sql: &str) {
        Self::record_sql(sql);
        with_conn(|conn| {
            conn.execute_batch(sql).expect("Db::exec");
            LAST_INSERT_ROWID.with(|c| c.set(conn.last_insert_rowid()));
        });
    }

    /// Begin recording every SQL string issued through `prepare` /
    /// `exec` on this thread, until `capture_sql_take`. Test-only
    /// instrument mirroring spinel's `Db.capture_sql` — the query
    /// counter that catches the `includes(:assoc)` N+1 (roundhouse#40,
    /// #27). See the `SQL_CAPTURE` thread-local for why thread-local.
    pub fn capture_sql_start() {
        SQL_CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
    }

    /// Stop recording and return the SQL captured since
    /// `capture_sql_start` (empty if capture was never started).
    pub fn capture_sql_take() -> Vec<String> {
        SQL_CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
    }

    fn record_sql(sql: &str) {
        SQL_CAPTURE.with(|c| {
            if let Some(log) = c.borrow_mut().as_mut() {
                log.push(sql.to_string());
            }
        });
    }

    /// Prepare a SELECT, materialize every row, return the opaque
    /// stmt id. Subsequent `step` / `column_*` / `finalize` calls take
    /// the id by value. `prepare_cached` reuses the connection's compiled
    /// statement (roundhouse#12) — the parse is skipped on a cache hit;
    /// rusqlite resets the cached statement on checkout and returns it to
    /// the LRU when the `CachedStatement` drops at the end of this closure.
    pub fn prepare(sql: &str) -> i64 {
        Self::record_sql(sql);
        let rows: Vec<Vec<Value>> = with_conn(|conn| {
            let mut stmt = conn.prepare_cached(sql).expect("Db::prepare");
            let n_cols = stmt.column_count();
            let mut out: Vec<Vec<Value>> = Vec::new();
            let mut rows = stmt.query([]).expect("Db::prepare query");
            while let Some(row) = rows.next().expect("Db::prepare step") {
                let mut col_vec = Vec::with_capacity(n_cols);
                for i in 0..n_cols {
                    let v: Value = row.get(i).expect("Db::prepare col");
                    col_vec.push(v);
                }
                out.push(col_vec);
            }
            out
        });
        let id = NEXT_STMT_ID.with(|c| {
            let n = c.get() + 1;
            c.set(n);
            n
        });
        STATEMENTS.with(|s| {
            s.borrow_mut().insert(
                id,
                StmtEntry { rows, pos: 0, current: None },
            );
        });
        id
    }

    /// Advance the cursor. Snapshots the current row into the entry
    /// and returns true; clears the snapshot + returns false when
    /// exhausted. Idempotent on unknown stmt ids (returns false).
    pub fn step(stmt_id: i64) -> bool {
        STATEMENTS.with(|s| {
            let mut map = s.borrow_mut();
            let Some(entry) = map.get_mut(&stmt_id) else { return false };
            if entry.pos < entry.rows.len() {
                entry.current = Some(entry.rows[entry.pos].clone());
                entry.pos += 1;
                true
            } else {
                entry.current = None;
                false
            }
        })
    }

    /// Read an integer column from the row most recently stepped.
    /// NULL coerces to 0 (matches Crystal/TS shims); non-Int variants
    /// best-effort coerce.
    pub fn column_int(stmt_id: i64, i: i64) -> i64 {
        STATEMENTS.with(|s| {
            let map = s.borrow();
            let Some(entry) = map.get(&stmt_id) else { return 0 };
            let Some(row) = entry.current.as_ref() else { return 0 };
            match row.get(i as usize) {
                Some(Value::Integer(v)) => *v,
                Some(Value::Real(v)) => *v as i64,
                Some(Value::Text(t)) => t.parse().unwrap_or(0),
                _ => 0,
            }
        })
    }

    /// Read a text column. NULL → "" (matches Crystal/TS); numeric
    /// variants stringify.
    pub fn column_text(stmt_id: i64, i: i64) -> String {
        STATEMENTS.with(|s| {
            let map = s.borrow();
            let Some(entry) = map.get(&stmt_id) else { return String::new() };
            let Some(row) = entry.current.as_ref() else { return String::new() };
            match row.get(i as usize) {
                Some(Value::Text(t)) => t.clone(),
                Some(Value::Integer(v)) => v.to_string(),
                Some(Value::Real(v)) => v.to_string(),
                Some(Value::Blob(b)) => String::from_utf8_lossy(b).into_owned(),
                _ => String::new(),
            }
        })
    }

    /// Read a boolean column. SQLite stores booleans as 0/1 integers
    /// — widen to bool. Nulls coerce to false.
    pub fn column_bool(stmt_id: i64, i: i64) -> bool {
        Self::column_int(stmt_id, i) != 0
    }

    /// Drop the stmt-table entry. Idempotent on unknown ids.
    pub fn finalize(stmt_id: i64) {
        STATEMENTS.with(|s| {
            s.borrow_mut().remove(&stmt_id);
        });
    }

    /// Last-row-id from the most recent `exec`. SQLite-specific.
    pub fn last_insert_rowid() -> i64 {
        LAST_INSERT_ROWID.with(|c| c.get())
    }

    /// SQL-quote a string literal. Single quotes doubled per SQLite's
    /// escape rule; no other byte transforms.
    pub fn escape_string(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for ch in s.chars() {
            if ch == '\'' {
                out.push_str("''");
            } else {
                out.push(ch);
            }
        }
        out.push('\'');
        out
    }

    /// Render an integer for SQL inlining.
    pub fn escape_int(n: i64) -> String {
        n.to_string()
    }

    /// Render an integer list for `IN (...)` eager-load batches (issue
    /// #27). An empty list yields "NULL" so `IN (NULL)` is valid SQL
    /// matching no rows — an empty `IN ()` is a syntax error.
    pub fn escape_int_list(ids: Vec<i64>) -> String {
        if ids.is_empty() {
            return "NULL".to_string();
        }
        ids.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
    }

    /// SQLite stores booleans as 0/1 integers.
    pub fn escape_bool(b: bool) -> String {
        (if b { "1" } else { "0" }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_SCHEMA: &str = r#"
CREATE TABLE widgets (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT
);
"#;

    #[test]
    fn setup_installs_connection_with_schema() {
        setup_test_db(TINY_SCHEMA);
        let row_count = with_conn(|c| {
            c.execute("INSERT INTO widgets (name) VALUES ('a'), ('b')", [])
                .expect("insert")
        });
        assert_eq!(row_count, 2);
        let count: i64 = with_conn(|c| {
            c.query_row("SELECT COUNT(*) FROM widgets", [], |r| r.get(0))
                .expect("count")
        });
        assert_eq!(count, 2);
    }

    #[test]
    fn production_pool_serves_parallel_readers() {
        // Force a 4-slot pool against per-conn `:memory:` databases.
        // Each slot is independent, so we use a query that doesn't
        // depend on shared state — purpose is to confirm `with_conn`
        // can hand out slots concurrently without deadlocking, and
        // that `try_lock` picks an idle slot under contention.
        open_production_pool(":memory:", TINY_SCHEMA, 4);

        let handles: Vec<_> = (0..16)
            .map(|i| {
                std::thread::spawn(move || {
                    // Read a literal so each slot's empty :memory: is fine.
                    let n: i64 = with_conn(|c| {
                        c.query_row("SELECT ?1 + 1", [i as i64], |r| r.get(0))
                            .expect("query")
                    });
                    assert_eq!(n, i as i64 + 1);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("worker join");
        }
    }

    #[test]
    fn setup_replaces_previous_connection() {
        setup_test_db(TINY_SCHEMA);
        with_conn(|c| {
            c.execute("INSERT INTO widgets (name) VALUES ('stale')", [])
                .expect("insert")
        });
        setup_test_db(TINY_SCHEMA);
        let count: i64 = with_conn(|c| {
            c.query_row("SELECT COUNT(*) FROM widgets", [], |r| r.get(0))
                .expect("count")
        });
        assert_eq!(count, 0, "new connection should start empty");
    }
}
