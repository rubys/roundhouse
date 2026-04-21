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
//!   - `open_production_db(path, schema)` — file-backed connection
//!     installed into a process-wide `Mutex<Option<Connection>>`.
//!     Used by `main.rs` on server startup. `with_conn` reaches
//!     either slot (test thread-local first, then process mutex).

use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

thread_local! {
    /// The connection the current thread's test (or request handler)
    /// uses. `None` until `setup_test_db` initializes it.
    static CONN: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

/// Process-wide connection for the production server. `axum`
/// handlers run on a multi-thread tokio runtime so per-thread
/// thread-locals don't work — instead a `Mutex<Connection>` lets
/// every handler serialize access to the single sqlite connection.
/// Contention is fine for the E2E scaffold; a connection pool is
/// the right answer for a production workload.
static PROD_CONN: Mutex<Option<Connection>> = Mutex::new(None);

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
            let guard = PROD_CONN.lock().expect("prod DB mutex poisoned");
            let conn = guard.as_ref().expect(
                "db not initialized; call setup_test_db or open_production_db first",
            );
            f(conn)
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
/// string. The transform to `CREATE TABLE IF NOT EXISTS` lets us
/// re-open an existing DB without crashing on duplicate-table
/// errors; the emitter produces plain `CREATE TABLE` today.
pub fn open_production_db(path: &str, schema_sql: &str) {
    if path != ":memory:" {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent).expect("mkdir db parent");
            }
        }
    }
    let conn = Connection::open(path).expect("open sqlite db");
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("enable WAL");
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign keys");
    let guarded = schema_sql.replace("CREATE TABLE ", "CREATE TABLE IF NOT EXISTS ");
    conn.execute_batch(&guarded).expect("apply schema");
    *PROD_CONN.lock().expect("prod DB mutex") = Some(conn);
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
