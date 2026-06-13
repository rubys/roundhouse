// Primitive Db surface — TypeScript / SharedWorker / MessagePort proxy.
//
// Third sibling of `db.ts` (sync better-sqlite3 wrap) and
// `db-libsql.ts` (async @libsql/client wrap). Same `Db` namespace
// export shape; the TypeScript emitter selects which file to inline
// based on the active deployment profile (sync → db.ts, libsql →
// db-libsql.ts, SharedWorker → this file), the same way `server.ts`
// and `juntos.ts` swap. See project_arel_compile_time_first.md.
//
// Why a third variant: the SharedWorker target runs the application
// tier inside a `SharedWorkerGlobalScope`, where neither better-sqlite3
// nor @libsql/client (both Node libraries) can load. sqlite-wasm with
// OPFS persistence can only run in a *dedicated* Worker
// (`db_worker.ts`), reached over a MessagePort. So this Db surface owns
// no database — it serializes each call into the db_worker message
// protocol (`{ id, type: 'exec', sql, params }` → `{ id, type:
// 'result', rows, changes, lastInsertRowId }`) and awaits the reply,
// exactly like `juntos-worker.ts`'s WorkerActiveRecordAdapter does for
// the higher-level adapter methods. The two share the same db_worker
// handle and the same reply stream; each keeps its own `_pending` map
// keyed by a unique `id`, so replies addressed to the other side are
// simply ignored (`handleWorkerMessage`'s `if (!resolver) return`).
//
// API shape matches db-libsql.ts:
//
//   - `exec(sql)` returns `Promise<void>`; stashes the reply's
//     `changes` / `lastInsertRowId` for the sync readers below.
//   - `prepare(sql)` returns `Promise<number>` and EAGERLY runs the
//     SELECT, caching the resulting rows (as ordered arrays in SELECT
//     column order) on the stmt entry. Subsequent `step?` /
//     `column_int` / `column_text` reads are synchronous iterations
//     over that cached array.
//   - `last_insert_rowid()` and `changes()` read from the most recent
//     `exec`/`prepare` reply — they stay sync.
//   - Everything else (`step?`, `column_*`, `finalize`, escape helpers)
//     is sync because the work happens entirely in the cached rows.
//
// The async surface (configure / exec / prepare) drives async coloring:
// `SqliteAsyncAdapter::async_seed_methods()` includes these names so
// the propagation pass marks any method calling them as async — the
// emitted models already `await Db.prepare(...)` / `await Db.exec(...)`.

// The db_worker handle: a `Worker` (Firefox direct spawn) or a
// `MessagePort` (Chrome tab-delegated spawn). Both expose
// postMessage / addEventListener / removeEventListener. Installed by
// `server-worker.ts` (`Db.install(_dbWorker!)`) right after it calls
// the framework adapter's `installDb(_dbWorker!)`.
type DbWorkerHandle = Pick<Worker, "postMessage" | "addEventListener" | "removeEventListener">;

interface ExecReply {
  rows: Record<string, unknown>[];
  changes: number;
  lastInsertRowId: number | null;
}

interface PendingResolver {
  resolve: (reply: ExecReply) => void;
  reject: (err: Error) => void;
}

type StmtEntry = {
  rows: unknown[][];
  cursor: number; // index of the row most recently surfaced via step?
};

let _dbWorker: DbWorkerHandle | null = null;
const _pending = new Map<string, PendingResolver>();
const _statements: Map<number, StmtEntry> = new Map();
let _nextId = 0;
let _lastReply: ExecReply | null = null;

function handleWorkerMessage(event: Event): void {
  const data = (event as MessageEvent).data as
    | { id?: string; type?: string; error?: string; rows?: Record<string, unknown>[]; changes?: number; lastInsertRowId?: number | null }
    | undefined;
  if (!data || !data.id) return;
  const resolver = _pending.get(data.id);
  if (!resolver) return; // not ours — belongs to the adapter's _pending
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

function sendExec(sql: string): Promise<ExecReply> {
  return new Promise((resolve, reject) => {
    if (!_dbWorker) {
      reject(new Error("Db not configured — call Db.install(dbWorker) first"));
      return;
    }
    const id = crypto.randomUUID();
    _pending.set(id, { resolve, reject });
    _dbWorker.postMessage({ id, type: "exec", sql, params: [] });
  });
}

// `configure(path)` is a no-op for the SharedWorker target: the
// dedicated db_worker is opened by `server-worker.ts`'s `sendDbInit`
// (an `init` message), not by this surface. Kept for namespace-shape
// parity with db.ts / db-libsql.ts so the emitter's call sites are
// identical across profiles.
async function configure(_path: string): Promise<void> {
  void _path;
}

/** Adopt the dedicated db_worker handle and start correlating its
 *  replies. Called by `server-worker.ts` once the Worker has replied
 *  `{ type: 'ready' }`. Additive listener — coexists with the
 *  framework adapter's own listener on the same handle. */
function install(worker: DbWorkerHandle): void {
  if (_dbWorker && _dbWorker !== worker) {
    _dbWorker.removeEventListener("message", handleWorkerMessage);
  }
  _dbWorker = worker;
  worker.addEventListener("message", handleWorkerMessage);
}

async function close(): Promise<void> {
  if (_dbWorker !== null) {
    _dbWorker.removeEventListener("message", handleWorkerMessage);
    _dbWorker = null;
  }
  _statements.clear();
  _pending.clear();
  _lastReply = null;
}

async function exec(sql: string): Promise<void> {
  _lastReply = await sendExec(sql);
}

// Eager-execute SELECT so subsequent step? / column_* reads can stay
// sync. db_worker returns object rows (`Record<string, unknown>`);
// sqlite-wasm's object rowMode preserves SELECT column order, so
// `Object.values(row)` yields the row in column-declaration order —
// matching what the lowerer-emitted column_int/column_text(stmt, i)
// index reads expect (same contract db-libsql.ts derives from
// `result.columns`).
async function prepare(sql: string): Promise<number> {
  const reply = await sendExec(sql);
  _lastReply = reply;
  const rows: unknown[][] = reply.rows.map((r) => Object.values(r));
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
  if (_lastReply === null) return 0;
  const v = _lastReply.lastInsertRowId;
  if (v === undefined || v === null) return 0;
  return typeof v === "bigint" ? Number(v) : v;
}

function changes(): number {
  return _lastReply?.changes ?? 0;
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
