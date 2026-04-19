//! Roundhouse Rust DB runtime.
//!
//! Hand-written helpers the Rust emitter copies verbatim into each
//! generated project as `src/db.rs`. Owns the per-test SQLite
//! connection and hides rusqlite borrowing from the generated code
//! — save/destroy/count/find all go through `with_conn`.
//!
//! Design: one thread-local `Option<Connection>` per test-thread.
//! Each test calls `setup_test_db(CREATE_TABLES_SQL)` at the top of
//! its body; that opens a fresh `:memory:` connection, runs the
//! schema DDL, and installs it into the cell, replacing anything
//! a previous test on the same thread left behind.
//!
//! Not `#[cfg(test)]` because Phase 4's production runtime will
//! layer a file-backed connection over the same `CONN` slot; the
//! public shape should stay stable across that move.

use std::cell::RefCell;

use rusqlite::Connection;

thread_local! {
    /// The connection the current thread's test (or request handler)
    /// uses. `None` until `setup_test_db` initializes it.
    static CONN: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

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

/// Borrow the current thread's connection. Panics if the connection
/// has not been installed — callers are generated code that runs
/// inside a test whose setup already called `setup_test_db`.
pub fn with_conn<R, F: FnOnce(&Connection) -> R>(f: F) -> R {
    CONN.with(|c| {
        let borrowed = c.borrow();
        let conn = borrowed
            .as_ref()
            .expect("test db not initialized; call setup_test_db first");
        f(conn)
    })
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
