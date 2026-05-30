// Primitive Db surface — TypeScript / Node / @libsql/client implementation.
//
// Async sibling of `db.ts` (sync better-sqlite3 wrap). Same `Db`
// namespace export shape; the TypeScript emitter selects which file
// to inline based on the active deployment profile (sync → db.ts,
// libsql → db-libsql.ts), the same way `server.ts` swaps with
// `server-libsql.ts`. See project_arel_compile_time_first.md.
//
// API differences from db.ts:
//
//   - `exec(sql)` returns `Promise<void>` (libsql's `client.execute`
//     is async-native).
//   - `prepare(sql)` returns `Promise<number>` and EAGERLY runs the
//     SELECT, caching the resulting rows on the stmt entry. Subsequent
//     `step?` / `column_int` / `column_text` reads are synchronous
//     iterations over that cached array.
//   - `last_insert_rowid()` and `changes()` read from the most recent
//     `exec` result that was stashed at await time — they stay sync.
//   - Everything else (`step?`, `column_*`, `finalize`, escape helpers)
//     is sync because the work happens entirely in the cached rows
//     after the prepare/exec await.
//
// The async surface (configure / exec / prepare) drives async coloring:
// `SqliteAsyncAdapter::async_seed_methods()` includes these names so
// the propagation pass marks any method calling them as async.

import { type Client, type InValue, type ResultSet } from "@libsql/client";

type StmtEntry = {
  rows: unknown[][];
  cursor: number; // index of the row most recently surfaced via step?
};

let _client: Client | null = null;
const _statements: Map<number, StmtEntry> = new Map();
let _nextId = 0;
let _lastResult: ResultSet | null = null;

function client(): Client {
  if (_client === null) throw new Error("Db not configured — call Db.configure(path) first");
  return _client;
}

async function configure(path: string): Promise<void> {
  // Lazy import keeps `@libsql/client` out of sync-profile bundles.
  const { createClient } = await import("@libsql/client");
  _client = createClient({ url: path === ":memory:" ? ":memory:" : `file:${path}` });
}

function install(adopted: Client): void {
  _client = adopted;
}

async function close(): Promise<void> {
  if (_client !== null) {
    _client.close();
    _client = null;
  }
}

async function exec(sql: string): Promise<void> {
  _lastResult = await client().execute(sql);
}

// Eager-execute SELECT so subsequent step? / column_* reads can stay
// sync. libsql returns row objects (Record<string, unknown>); rows in
// row-array form preserve column order, which matches what the
// lowerer-emitted column_int/column_text(stmt, i) reads expect.
async function prepare(sql: string): Promise<number> {
  const result = await client().execute(sql);
  _lastResult = result;
  // Convert row-objects to ordered arrays in column-declaration order
  // (result.columns gives the column names in SELECT order).
  const cols = result.columns;
  const rows: unknown[][] = result.rows.map((r) => {
    const rec = r as Record<string, unknown>;
    return cols.map((c) => rec[c]);
  });
  _nextId += 1;
  _statements.set(_nextId, { rows, cursor: -1 });
  return _nextId;
}

function is_step(stmtId: number): boolean {
  const entry = _statements.get(stmtId);
  if (entry === undefined) throw new Error(`Db: unknown stmt id ${stmtId}`);
  entry.cursor += 1;
  return entry.cursor < entry.rows.length;
}

function column_int(stmtId: number, i: number): number {
  const entry = _statements.get(stmtId);
  if (entry === undefined || entry.cursor < 0 || entry.cursor >= entry.rows.length) {
    throw new Error(`Db: column_int called on stmt ${stmtId} with no current row`);
  }
  const v = entry.rows[entry.cursor][i];
  if (v === null || v === undefined) return 0;
  if (typeof v === "bigint") return Number(v);
  return typeof v === "number" ? Math.trunc(v) : Number(v) | 0;
}

function column_text(stmtId: number, i: number): string {
  const entry = _statements.get(stmtId);
  if (entry === undefined || entry.cursor < 0 || entry.cursor >= entry.rows.length) {
    throw new Error(`Db: column_text called on stmt ${stmtId} with no current row`);
  }
  const v = entry.rows[entry.cursor][i];
  if (v === null || v === undefined) return "";
  return String(v);
}

function finalize(stmtId: number): void {
  _statements.delete(stmtId);
}

function last_insert_rowid(): number {
  if (_lastResult === null) return 0;
  const v = _lastResult.lastInsertRowid;
  if (v === undefined || v === null) return 0;
  return typeof v === "bigint" ? Number(v) : v;
}

function changes(): number {
  return _lastResult?.rowsAffected ?? 0;
}

function escape_string(s: unknown): string {
  return "'" + String(s ?? "").replace(/'/g, "''") + "'";
}

function escape_int(n: unknown): string {
  const parsed = typeof n === "number" ? n : Number(n);
  return Number.isFinite(parsed) ? String(Math.trunc(parsed)) : "0";
}

function escape_int_list(ids: unknown[]): string {
  // `IN (...)` eager-load batches (issue #27); empty → "NULL".
  if (ids.length === 0) return "NULL";
  return ids.map((n) => escape_int(n)).join(", ");
}

function escape_bool(b: unknown): string {
  return b ? "1" : "0";
}

function column_bool(stmtId: number, i: number): boolean {
  return column_int(stmtId, i) !== 0;
}

// Suppress an unused-import warning when InValue isn't referenced
// directly anywhere in this file (libsql types it via `args: InValue[]`
// inside execute; we don't bind params, only inline-compose SQL).
const _unused: InValue | undefined = undefined;
void _unused;

// Names match the TypeScript emitter's Ruby→TS rename rule (see
// `src/emit/typescript/naming.rs::ts_method_name`): Ruby's `?` suffix
// becomes an `is_` prefix at the call site, so `Db.step?(stmt)` emits
// as `Db.is_step(stmt)`.
export const Db = {
  configure,
  install,
  close,
  exec,
  prepare,
  is_step,
  column_int,
  column_text,
  column_bool,
  finalize,
  last_insert_rowid,
  changes,
  escape_string,
  escape_int,
  escape_int_list,
  escape_bool,
};

export type DbModule = typeof Db;
