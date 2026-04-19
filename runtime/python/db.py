"""Roundhouse Python DB runtime.

Hand-written helpers the Python emitter copies verbatim into each
generated project as `app/db.py`. Uses the stdlib `sqlite3` module,
so generated projects have zero non-Python runtime dependencies.

Connection lives in a module-level variable. Each test's `setUp`
calls `setup_test_db` to open a fresh `:memory:` connection with the
schema DDL applied, replacing whatever the previous test left
behind.
"""

from __future__ import annotations

import sqlite3
from typing import Any

_conn: sqlite3.Connection | None = None


def setup_test_db(schema_sql: str) -> None:
    """Open a fresh :memory: SQLite connection, run the schema DDL,
    and install it in the module-level slot. `sqlite3`'s
    `executescript` handles multi-statement batches natively, so no
    per-statement splitting needed.
    """
    global _conn
    if _conn is not None:
        _conn.close()
    _conn = sqlite3.connect(":memory:")
    _conn.row_factory = sqlite3.Row
    _conn.executescript(schema_sql)


def conn() -> sqlite3.Connection:
    """Borrow the current test's connection. Raises if unset."""
    if _conn is None:
        raise RuntimeError(
            "test db not initialized; call setup_test_db(create_tables) first"
        )
    return _conn


def execute(sql: str, params: list[Any] | None = None) -> int:
    """Run a mutating statement (INSERT / UPDATE / DELETE). Returns
    the rowid of the last insert on the connection — useful for
    INSERTs, ignored by callers for other operations.
    """
    cur = conn().execute(sql, params or [])
    rowid = cur.lastrowid or 0
    cur.close()
    return rowid


def query_one(sql: str, params: list[Any] | None = None) -> sqlite3.Row | None:
    """Run a single-row SELECT; returns the `Row` (dict-like) or
    `None` when the query found nothing.
    """
    cur = conn().execute(sql, params or [])
    row = cur.fetchone()
    cur.close()
    return row


def query_all(sql: str, params: list[Any] | None = None) -> list[sqlite3.Row]:
    """Run a multi-row SELECT; returns a list of `Row`s."""
    cur = conn().execute(sql, params or [])
    rows = cur.fetchall()
    cur.close()
    return rows


def scalar(sql: str, params: list[Any] | None = None) -> Any:
    """Scalar query — first column of the first row."""
    row = query_one(sql, params)
    return row[0] if row is not None else None
