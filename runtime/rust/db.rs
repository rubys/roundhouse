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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::types::Value;
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
/// string. The emitter produces `CREATE TABLE IF NOT EXISTS`
/// directly, so re-opening an existing DB no-ops over the
/// already-present tables.
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
    conn.execute_batch(schema_sql).expect("apply schema");
    *PROD_CONN.lock().expect("prod DB mutex") = Some(conn);
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
        with_conn(|conn| {
            conn.execute_batch(sql).expect("Db::exec");
            LAST_INSERT_ROWID.with(|c| c.set(conn.last_insert_rowid()));
        });
    }

    /// Prepare a SELECT, materialize every row, return the opaque
    /// stmt id. Subsequent `step` / `column_*` / `finalize` calls take
    /// the id by value.
    pub fn prepare(sql: &str) -> i64 {
        let rows: Vec<Vec<Value>> = with_conn(|conn| {
            let mut stmt = conn.prepare(sql).expect("Db::prepare");
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
