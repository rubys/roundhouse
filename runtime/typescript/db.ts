// Primitive Db surface — TypeScript / Node / better-sqlite3 implementation.
//
// Mirrors `runtime/spinel/db.rb`'s contract verbatim: the lowerer emits
// per-model `_adapter_*` methods that compose SQL strings and dispatch
// against this surface. Sibling shims (cruby/sqlite-gem under spinel,
// libsql for async-target, postgres/etc.) implement the same module
// name; per-database SQL dialect differences live in a separate dialect
// helper consulted at SQL composition time. See
// project_level_3_adapter_emit.md and project_arel_compile_time_first.md.
//
// API (every Db shim must satisfy this):
//
//   Db.configure(path)         — open a database (":memory:" for tests)
//   Db.close()                 — close the database
//   Db.exec(sql)               — run DDL / INSERT / UPDATE / DELETE
//   Db.prepare(sql)            — prepare a SELECT, returns stmt handle
//   Db.step?(stmt)             — advance, returns true if a row arrived
//   Db.column_int(stmt, i)     — read int column at zero-based index
//   Db.column_text(stmt, i)    — read text column at zero-based index
//   Db.finalize(stmt)          — release the prepared stmt
//   Db.last_insert_rowid()     — id of the last INSERTed row
//   Db.changes()               — affected-row count of the last statement
//   Db.escape_string(s)        — SQL-quote a string value
//   Db.escape_int(n)           — render an integer for SQL inlining
//
// Stmt handles are opaque integers indexing into a per-process table
// that caches each prepared statement's iterator + most recently
// stepped row. The cache lets `column_int` / `column_text` pick fields
// by index without re-stepping.
//
// Per-shape question-mark methods (`step?`) emit as
// `["step?"]` in TypeScript since Ruby permits `?` in method names but
// JS does not. Lowered call sites use the same bracket form, so the
// shim exposes them under their original names via the bracket-export
// shape below.

import Database from "better-sqlite3";

type StmtEntry = {
  iterator: IterableIterator<unknown[]>;
  current: unknown[] | undefined;
};

let _db: Database.Database | null = null;
const _statements: Map<number, StmtEntry> = new Map();
let _nextId = 0;
let _lastRunResult: { lastInsertRowid: number | bigint; changes: number } | null = null;

function db(): Database.Database {
  if (_db === null) throw new Error("Db not configured — call Db.configure(path) first");
  return _db;
}

function configure(path: string): void {
  _db = new Database(path);
}

// Adopt an already-opened Database. Lets the test runtime share one
// in-memory connection between the legacy juntos `installDb(db)` path
// and this Level-3 Db primitive surface — without this, Level-3
// reads would miss anything written by legacy AR helpers (and vice
// versa) because each call would have opened a separate `:memory:`
// DB. The setup.ts emitter calls both `installDb(db)` and
// `Db.install(db)` so both paths see the same rows.
function install(adopted: Database.Database): void {
  _db = adopted;
}

function close(): void {
  if (_db !== null) {
    _db.close();
    _db = null;
  }
}

// Run any one-shot SQL: DDL, INSERT, UPDATE, DELETE. better-sqlite3's
// `Database#exec` runs multi-statement SQL but doesn't expose
// last_insert_rowid; for the AR contract we route everything through
// prepare+run so `last_insert_rowid` after an INSERT gives the right
// value. DDL (CREATE TABLE etc.) tolerates this routing fine.
function exec(sql: string): void {
  const stmt = db().prepare(sql);
  _lastRunResult = stmt.run();
}

function prepare(sql: string): number {
  // `.raw(true)` switches Statement.iterate() to yield arrays instead
  // of objects. Index-based column reads (column_int, column_text)
  // line up with this array form.
  const stmt = db().prepare(sql).raw(true);
  const iterator = stmt.iterate() as IterableIterator<unknown[]>;
  _nextId += 1;
  _statements.set(_nextId, { iterator, current: undefined });
  return _nextId;
}

function stepQ(stmtId: number): boolean {
  const entry = _statements.get(stmtId);
  if (entry === undefined) throw new Error(`Db: unknown stmt id ${stmtId}`);
  const result = entry.iterator.next();
  if (result.done) {
    entry.current = undefined;
    return false;
  }
  entry.current = result.value;
  return true;
}

function columnInt(stmtId: number, i: number): number {
  const entry = _statements.get(stmtId);
  if (entry === undefined || entry.current === undefined) {
    throw new Error(`Db: column_int called on stmt ${stmtId} with no current row`);
  }
  const v = entry.current[i];
  if (v === null || v === undefined) return 0;
  return typeof v === "number" ? Math.trunc(v) : Number(v) | 0;
}

function columnText(stmtId: number, i: number): string {
  const entry = _statements.get(stmtId);
  if (entry === undefined || entry.current === undefined) {
    throw new Error(`Db: column_text called on stmt ${stmtId} with no current row`);
  }
  const v = entry.current[i];
  if (v === null || v === undefined) return "";
  return String(v);
}

// Drain any unread rows so better-sqlite3 releases the underlying
// statement cleanly; iterators that haven't been exhausted leave
// statements in a "busy" state across Database#close.
function finalize(stmtId: number): void {
  const entry = _statements.get(stmtId);
  if (entry === undefined) return;
  try {
    while (!entry.iterator.next().done) { /* drain */ }
  } catch {
    /* iterator already exhausted or errored — nothing to release */
  }
  _statements.delete(stmtId);
}

function lastInsertRowid(): number {
  if (_lastRunResult === null) return 0;
  const v = _lastRunResult.lastInsertRowid;
  return typeof v === "bigint" ? Number(v) : v;
}

function changes(): number {
  return _lastRunResult?.changes ?? 0;
}

function escapeString(s: unknown): string {
  return "'" + String(s ?? "").replace(/'/g, "''") + "'";
}

function escapeInt(n: unknown): string {
  // Truncate-to-int matches Ruby's `n.to_i.to_s` semantic in
  // `runtime/spinel/db.rb`. Non-numeric input parses to 0 (Ruby
  // semantics: "abc".to_i == 0).
  const parsed = typeof n === "number" ? n : Number(n);
  return Number.isFinite(parsed) ? String(Math.trunc(parsed)) : "0";
}

function escapeIntList(ids: unknown[]): string {
  // Render an integer list for `IN (...)` eager-load batches (issue
  // #27). Empty list → "NULL" so `IN (NULL)` is valid SQL matching no
  // rows (an empty `IN ()` is a syntax error).
  if (ids.length === 0) return "NULL";
  return ids.map((n) => escapeInt(n)).join(", ");
}

function escapeBool(b: unknown): string {
  // SQLite stores booleans as 0/1 integers — mirror the Ruby/Crystal
  // sibling shims. Truthiness check accepts Ruby's `truthy` interp.
  return b ? "1" : "0";
}

function columnBool(stmtId: number, idx: number): boolean {
  return columnInt(stmtId, idx) !== 0;
}

// Method names match the TypeScript emitter's Ruby→TS rename rule
// (see `src/emit/typescript/naming.rs::ts_method_name`): Ruby's `?`
// suffix becomes an `is_` prefix at the call site, so `Db.step?(stmt)`
// emits as `Db.is_step(stmt)` and the export is named accordingly.
// All other names preserve their snake_case shape verbatim.
export const Db = {
  configure,
  install,
  close,
  exec,
  prepare,
  is_step: stepQ,
  column_int: columnInt,
  column_text: columnText,
  column_bool: columnBool,
  finalize,
  last_insert_rowid: lastInsertRowid,
  changes,
  escape_string: escapeString,
  escape_int: escapeInt,
  escape_int_list: escapeIntList,
  escape_bool: escapeBool,
};

export type DbModule = typeof Db;
