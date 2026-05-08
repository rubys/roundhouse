// Roundhouse TypeScript primitive-runtime layer — SharedWorker variant.
//
// Mirrors `juntos-libsql.ts` but bridges the framework's 12-method
// `ActiveRecordAdapter` surface to the dedicated database Worker via
// MessagePort instead of speaking SQLite directly. The two files
// share `Row`, `Conditions`, `AdapterSchema`, `ForeignKey`, and the
// `ActiveRecordAdapter` interface — so transpiled framework code
// (Base.find/all/where/...) doesn't care which one the emit pipeline
// picked.
//
// The single semantic difference from `juntos-libsql.ts`: every
// `execute` round-trips a postMessage to the dedicated `db_worker.ts`
// instead of calling libsql in-process. The async-coloring pass in
// roundhouse already produces transpiled framework Ruby as `async`
// methods that `await adapter.X(...)` — this runtime swap is what
// makes those awaits load-bearing across the MessagePort boundary.
//
// Broadcasting: Action Cable's `broadcast(stream, html)` is backed
// by `BroadcastChannel` — natively reaches all tabs sharing the
// SharedWorker, no WebSocket needed.

import { ActiveRecord } from "./active_record_base.js";

// ── Shared types (kept in sync with juntos.ts / juntos-libsql.ts) ──

/** A single row as plain primitives. */
export type Row = Record<string, string | number | null>;

/** Equality conditions for `where(table, conditions)`. */
export type Conditions = Record<string, string | number | null>;

export type ForeignKey = { column: string; references: string };
export interface AdapterSchema {
  columns: string[];
  foreign_keys: ForeignKey[];
}

export interface ActiveRecordAdapter {
  // DDL
  create_table(name: string, columns: string[], foreign_keys?: ForeignKey[]): void | Promise<void>;
  drop_table(name: string): void | Promise<void>;
  schema(table: string): (AdapterSchema | null) | Promise<AdapterSchema | null>;
  // Read
  find(table: string, id: number): (Row | null) | Promise<Row | null>;
  all(table: string): Row[] | Promise<Row[]>;
  where(table: string, conditions: Conditions): Row[] | Promise<Row[]>;
  count(table: string): number | Promise<number>;
  is_exists(table: string, id: number): boolean | Promise<boolean>;
  // Write
  insert(table: string, row: Row): number | Promise<number>;
  update(table: string, id: number, row: Row): boolean | Promise<boolean>;
  delete(table: string, id: number): boolean | Promise<boolean>;
}

// ── MessagePort transport to db_worker ──
//
// `db_worker.ts` accepts `{ id, type, ...payload }` and replies with
// `{ id, type: 'result' | 'error', ... }`. We keep a `Map<id,
// resolver>` and correlate responses to the awaited Promises.

interface ExecResult {
  rows: Record<string, unknown>[];
  changes: number;
  lastInsertRowId: number | null;
}

interface PendingResolver {
  resolve: (result: ExecResult) => void;
  reject: (err: Error) => void;
}

type DbWorkerHandle = Pick<Worker, "postMessage" | "addEventListener" | "removeEventListener">;

let _dbWorker: DbWorkerHandle | null = null;
const _pending = new Map<string, PendingResolver>();

function handleWorkerMessage(event: Event): void {
  const data = (event as MessageEvent).data as
    | { id?: string; type?: string; error?: string; rows?: Record<string, unknown>[]; changes?: number; lastInsertRowId?: number | null }
    | undefined;
  if (!data || !data.id) return;
  const resolver = _pending.get(data.id);
  if (!resolver) return;
  _pending.delete(data.id);
  if (data.type === "error") {
    resolver.reject(new Error(data.error ?? "db_worker error"));
  } else {
    resolver.resolve({
      rows: data.rows ?? [],
      changes: data.changes ?? 0,
      lastInsertRowId: data.lastInsertRowId ?? null,
    });
  }
}

function sendMessage(message: Record<string, unknown>): Promise<ExecResult> {
  return new Promise((resolve, reject) => {
    if (!_dbWorker) {
      reject(new Error("db_worker not installed — call installDb() first"));
      return;
    }
    const id = crypto.randomUUID();
    _pending.set(id, { resolve, reject });
    _dbWorker.postMessage({ ...message, id });
  });
}

// ── Public surface ──

/** Install the dedicated database Worker handle and point the
 *  framework's `ActiveRecord.adapter` at a worker-backed adapter.
 *  Called by `Application.start` in `rails-worker.ts` once the
 *  Worker has replied with `{ type: 'ready' }`. */
export function installDb(worker: DbWorkerHandle): void {
  if (_dbWorker && _dbWorker !== worker) {
    _dbWorker.removeEventListener("message", handleWorkerMessage);
  }
  _dbWorker = worker;
  worker.addEventListener("message", handleWorkerMessage);
  ActiveRecord.adapter = new WorkerActiveRecordAdapter();
}

/** Signature for the broadcaster. Worker target backs this with a
 *  `BroadcastChannel` per stream — natively reaches all tabs
 *  sharing the SharedWorker, no WebSocket required. */
export type Broadcaster = (stream: string, html: string) => void;

let broadcaster: Broadcaster | null = null;

const _channels = new Map<string, BroadcastChannel>();

function getChannel(name: string): BroadcastChannel {
  let ch = _channels.get(name);
  if (!ch) {
    ch = new BroadcastChannel(name);
    _channels.set(name, ch);
  }
  return ch;
}

/** Default broadcaster — `BroadcastChannel.postMessage(html)`.
 *  Installed automatically; override via `setBroadcaster` for tests. */
const defaultBroadcaster: Broadcaster = (stream, html) => {
  getChannel(stream).postMessage(html);
};

export function setBroadcaster(fn: Broadcaster | null): void {
  broadcaster = fn;
}

export function broadcast(stream: string, html: string): void {
  (broadcaster ?? defaultBroadcaster)(stream, html);
}

// ── Worker-backed adapter ──

/** Translates each of the 12 framework adapter methods into a SQL
 *  string + positional params, posts to db_worker, awaits the
 *  result. No dialect logic here — the framework runtime owns query
 *  construction; this layer is the MessagePort boundary. */
export class WorkerActiveRecordAdapter implements ActiveRecordAdapter {
  async create_table(
    name: string,
    columns: string[],
    _foreign_keys: ForeignKey[] = [],
  ): Promise<void> {
    void name; void columns;
    // Schema DDL is applied by `Application.start` via `execSQL`
    // before `installDb`, so this is a deliberate no-op.
  }

  async drop_table(name: string): Promise<void> {
    await sendMessage({ type: "exec", sql: `DROP TABLE IF EXISTS ${name}`, params: [] });
  }

  async schema(table: string): Promise<AdapterSchema | null> {
    const result = await sendMessage({
      type: "exec",
      sql: `SELECT name FROM pragma_table_info(?)`,
      params: [table],
    });
    if (result.rows.length === 0) return null;
    return {
      columns: result.rows.map((r) => String(r.name)),
      foreign_keys: [],
    };
  }

  async find(table: string, id: number): Promise<Row | null> {
    const result = await sendMessage({
      type: "exec",
      sql: `SELECT * FROM ${table} WHERE id = ?`,
      params: [id],
    });
    return (result.rows[0] as Row | undefined) ?? null;
  }

  async all(table: string): Promise<Row[]> {
    const result = await sendMessage({
      type: "exec",
      sql: `SELECT * FROM ${table}`,
      params: [],
    });
    return result.rows as unknown as Row[];
  }

  async where(table: string, conditions: Conditions): Promise<Row[]> {
    const entries = Object.entries(conditions);
    if (entries.length === 0) return this.all(table);
    const clause = entries.map(([k]) => `${k} = ?`).join(" AND ");
    const values = entries.map(([, v]) => v);
    const result = await sendMessage({
      type: "exec",
      sql: `SELECT * FROM ${table} WHERE ${clause}`,
      params: values,
    });
    return result.rows as unknown as Row[];
  }

  async count(table: string): Promise<number> {
    const result = await sendMessage({
      type: "exec",
      sql: `SELECT COUNT(*) AS c FROM ${table}`,
      params: [],
    });
    const row = result.rows[0] as Record<string, unknown> | undefined;
    return Number(row?.c ?? 0);
  }

  async is_exists(table: string, id: number): Promise<boolean> {
    const found = await this.find(table, id);
    return found !== null;
  }

  async insert(table: string, row: Row): Promise<number> {
    const cols = Object.keys(row);
    if (cols.length === 0) {
      const result = await sendMessage({
        type: "exec",
        sql: `INSERT INTO ${table} DEFAULT VALUES`,
        params: [],
      });
      return Number(result.lastInsertRowId ?? 0);
    }
    const placeholders = cols.map(() => "?").join(", ");
    const values = cols.map((c) => row[c]);
    const result = await sendMessage({
      type: "exec",
      sql: `INSERT INTO ${table} (${cols.join(", ")}) VALUES (${placeholders})`,
      params: values,
    });
    return Number(result.lastInsertRowId ?? 0);
  }

  async update(table: string, id: number, row: Row): Promise<boolean> {
    const cols = Object.keys(row).filter((c) => c !== "id");
    if (cols.length === 0) return true;
    const sets = cols.map((c) => `${c} = ?`).join(", ");
    const values = cols.map((c) => row[c]);
    const result = await sendMessage({
      type: "exec",
      sql: `UPDATE ${table} SET ${sets} WHERE id = ?`,
      params: [...values, id],
    });
    return result.changes > 0;
  }

  async delete(table: string, id: number): Promise<boolean> {
    const result = await sendMessage({
      type: "exec",
      sql: `DELETE FROM ${table} WHERE id = ?`,
      params: [id],
    });
    return result.changes > 0;
  }
}

// ── Migration helpers ──
//
// Mirror the surface that `Application.runMigrations` calls in the
// libsql variant — `query` returns rows, `execute` returns
// `{ changes }`. Both round-trip through db_worker.

export async function query(sql: string, params: unknown[] = []): Promise<Record<string, unknown>[]> {
  const result = await sendMessage({ type: "exec", sql, params });
  return result.rows;
}

export async function execute(sql: string, params: unknown[] = []): Promise<{ changes: number }> {
  const result = await sendMessage({ type: "exec", sql, params });
  return { changes: result.changes };
}

/** Apply a multi-statement schema dump (CREATE TABLE / CREATE INDEX
 *  / etc.). Used at SharedWorker startup before `installDb`. */
export async function execSQL(sql: string): Promise<void> {
  await sendMessage({ type: "execSQL", sql });
}
