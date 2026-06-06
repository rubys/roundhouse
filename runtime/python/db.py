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


def open_production_db(path: str, schema_sql: str) -> None:
    """Open a file-backed SQLite connection, apply the schema DDL,
    and install the connection in the module-level slot. Used by
    `server.start` at process boot so each request can query
    through `conn()`. Skips the schema run when the target table(s)
    already exist, so an externally seeded DB isn't clobbered."""
    import os
    global _conn
    if _conn is not None:
        _conn.close()
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    _conn = sqlite3.connect(path, check_same_thread=False)
    _conn.row_factory = sqlite3.Row
    cur = _conn.execute(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"
    )
    tables = cur.fetchone()[0]
    cur.close()
    if tables == 0:
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


class Db:
    """Prepared-statement primitive for the transpiled ActiveRecord
    layer. The lowered per-model `_adapter_*` methods inline values into
    SQL strings (via `escape_string`/`escape_int`) and drive a cursor
    through `prepare` -> `step?` -> `column_int`/`column_text` ->
    `finalize`, exactly as the go (`db.go`) and rust (`db.rs`) twins do.

    `prepare` materializes every row up front and returns an opaque int
    id; `step?`/`column_*`/`finalize` take that id by value. Pre-fetching
    (rather than holding a live cursor) keeps the handle a plain int,
    which is what the lowered IR expects (`from_stmt(stmt: int)`), and
    sidesteps cursor-lifetime questions. Shares the module-level
    connection via `conn()` with the legacy helpers above.
    """

    _statements: dict[int, dict[str, Any]] = {}
    _next_id: int = 0
    _last_rowid: int = 0

    @classmethod
    def exec(cls, query: str) -> None:
        """Run a mutating statement (INSERT/UPDATE/DELETE), stashing the
        last insert rowid for a following `last_insert_rowid` call."""
        cur = conn().execute(query)
        cls._last_rowid = cur.lastrowid or 0
        cur.close()

    @classmethod
    def prepare(cls, query: str) -> int:
        """Run a SELECT, materialize all rows, and return an opaque
        statement id for stepping."""
        cur = conn().execute(query)
        rows = cur.fetchall()
        cur.close()
        cls._next_id += 1
        sid = cls._next_id
        cls._statements[sid] = {"rows": rows, "pos": 0, "current": None}
        return sid

    @classmethod
    def step_p(cls, stmt: int) -> bool:
        """Advance the cursor, snapshotting the next row. False when
        exhausted or on an unknown id (idempotent)."""
        entry = cls._statements.get(stmt)
        if entry is None:
            return False
        if entry["pos"] < len(entry["rows"]):
            entry["current"] = entry["rows"][entry["pos"]]
            entry["pos"] += 1
            return True
        entry["current"] = None
        return False

    @classmethod
    def column_int(cls, stmt: int, i: int) -> int:
        """Integer column of the most recently stepped row. NULL -> 0;
        text/float best-effort coerce (matches the go/rust shims)."""
        entry = cls._statements.get(stmt)
        if entry is None or entry["current"] is None:
            return 0
        v = entry["current"][i]
        if v is None:
            return 0
        try:
            return int(v)
        except (TypeError, ValueError):
            return 0

    @classmethod
    def column_text(cls, stmt: int, i: int) -> str:
        """Text column of the most recently stepped row. NULL -> ''."""
        entry = cls._statements.get(stmt)
        if entry is None or entry["current"] is None:
            return ""
        v = entry["current"][i]
        return "" if v is None else str(v)

    @classmethod
    def finalize(cls, stmt: int) -> None:
        """Drop the statement entry. Idempotent on unknown ids."""
        cls._statements.pop(stmt, None)

    @classmethod
    def last_insert_rowid(cls) -> int:
        """Rowid from the most recent `exec`."""
        return cls._last_rowid

    @staticmethod
    def escape_string(s: str) -> str:
        """SQL-quote a string literal (SQLite rule: single quotes
        doubled). Values are inlined into SQL, not bound."""
        return "'" + str(s).replace("'", "''") + "'"

    @staticmethod
    def escape_int(n: int) -> str:
        """Render an integer for SQL inlining."""
        return str(int(n))
