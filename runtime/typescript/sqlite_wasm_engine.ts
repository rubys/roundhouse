// Roundhouse TypeScript worker runtime — sqlite-wasm engine.
//
// Runs inside the dedicated database Worker (loaded by `db_worker.ts`).
// Wraps `@sqlite.org/sqlite-wasm`'s `oo1.DB` with a minimal raw-SQL
// surface — the SharedWorker tier's `WorkerActiveRecordAdapter`
// translates the framework's 12-method adapter calls into SQL strings
// and posts them here over MessagePort.
//
// Persistence: tries the `opfs-sahpool` VFS first (fast, synchronous
// I/O via `FileSystemSyncAccessHandle`, no COOP/COEP headers required).
// Falls back to in-memory `sqlite3.oo1.DB` if OPFS is unavailable.
//
// Why not the 12-method `ActiveRecordAdapter` shape here: db_worker is
// a SQL relay, not an ORM. Keeping it raw-SQL means migrations,
// schemas, queries, and DDL all flow through the same `exec` path with
// no per-shape branching in the dedicated worker.

// Minimal type declarations for `@sqlite.org/sqlite-wasm`. The
// package ships its own `.d.ts` but we only depend on the surface
// we actually call — declaring it locally keeps the transitive
// type surface bounded.
interface Sqlite3DB {
  exec(opts: {
    sql: string;
    bind?: unknown[];
    rowMode?: "object" | "array";
    callback?: (row: Record<string, unknown> | unknown[]) => void;
  }): void;
  changes(): number;
  close(): void;
}

interface OpfsSAHPoolUtil {
  OpfsSAHPoolDb: new (path: string) => Sqlite3DB;
}

interface Sqlite3 {
  oo1: {
    DB: new () => Sqlite3DB;
    OpfsDb?: new (path: string) => Sqlite3DB;
  };
  installOpfsSAHPoolVfs(opts: {
    initialCapacity?: number;
    clearOnInit?: boolean;
  }): Promise<OpfsSAHPoolUtil>;
}

// ── Module-level state ──

let _db: Sqlite3DB | null = null;

// ── Public surface (called by db_worker.ts) ──

export interface InitOptions {
  /** Logical database name (used as the OPFS file path). Default: `app.sqlite3`. */
  database?: string;
  /** Try OPFS persistence. Default: `true`. */
  opfs?: boolean;
}

/** Open the database. Tries opfs-sahpool first (worker-only,
 *  no isolation headers needed), falls back to in-memory. */
export async function initDatabase(options: InitOptions = {}): Promise<void> {
  // Idempotent: a DB worker reused across a SharedWorker hot-swap (the studio's
  // worker-reconnect loop) receives a fresh `init` from the new app instance;
  // re-opening the already-open opfs-sahpool would fight its own exclusive sync
  // access handles. First-open path is unchanged (_db starts null).
  if (_db) return;
  const { opfs = true, database: dbName = "app.sqlite3" } = options;

  const sqlite3InitModule =
    (await import("@sqlite.org/sqlite-wasm")).default as () => Promise<Sqlite3>;
  const sqlite3 = await sqlite3InitModule();

  const inWorker =
    typeof (globalThis as { WorkerGlobalScope?: unknown }).WorkerGlobalScope !== "undefined";

  if (opfs && inWorker) {
    try {
      // Namespace the OPFS-SAHPool VFS + directory per deploy path so two apps
      // on the same origin (e.g. /blog/ and the studio /studio/app/ instance)
      // never share a pool — a shared pool would have each app's SAHPool fight
      // over the same sync access handles and clobber the other's data.
      // BASE_URL is the deploy path (vite/esbuild-injected); "/" → the default.
      const base =
        (import.meta as unknown as { env?: { BASE_URL?: string } }).env?.BASE_URL ?? "/";
      const ns = base === "/" ? "" : base.replace(/[^A-Za-z0-9]+/g, "_").replace(/^_|_$/g, "");
      const pool = await sqlite3.installOpfsSAHPoolVfs({
        ...(ns ? { name: "opfs-sahpool-" + ns, directory: "." + ns + "-sahpool" } : {}),
        initialCapacity: 6,
        clearOnInit: false,
      });
      _db = new pool.OpfsSAHPoolDb("/" + dbName);
      return;
    } catch {
      // OPFS unavailable in this worker — fall through to in-memory
    }
  }

  _db = new sqlite3.oo1.DB();
}

/** Execute a single SQL statement with optional positional parameters.
 *  Returns rows for SELECT/PRAGMA, otherwise `{ changes, lastInsertRowId }`. */
export interface ExecResult {
  rows: Record<string, unknown>[];
  changes: number;
  lastInsertRowId: number | null;
}

export function exec(sql: string, params: unknown[] = []): ExecResult {
  const db = requireDb();
  const trimmed = sql.trim().toUpperCase();
  const isSelect = trimmed.startsWith("SELECT") || trimmed.startsWith("PRAGMA");

  if (isSelect) {
    const rows: Record<string, unknown>[] = [];
    db.exec({
      sql,
      bind: params,
      rowMode: "object",
      callback: (row) => {
        rows.push(row as Record<string, unknown>);
      },
    });
    return { rows, changes: 0, lastInsertRowId: null };
  }

  db.exec({ sql, bind: params });
  const changes = db.changes();

  let lastInsertRowId: number | null = null;
  if (trimmed.startsWith("INSERT")) {
    const lastId: unknown[] = [];
    db.exec({
      sql: "SELECT last_insert_rowid()",
      rowMode: "array",
      callback: (row) => {
        lastId.push((row as unknown[])[0]);
      },
    });
    lastInsertRowId = lastId[0] != null ? Number(lastId[0]) : null;
  }

  return { rows: [], changes, lastInsertRowId };
}

/** Execute a multi-statement SQL string (for schema dumps / migrations).
 *  Splits on `;`, trims, skips empty fragments. */
export function execSQL(sql: string): void {
  const db = requireDb();
  for (const stmt of sql.split(";")) {
    const trimmed = stmt.trim();
    if (trimmed) db.exec({ sql: trimmed });
  }
}

export function closeDatabase(): void {
  if (_db) {
    try { _db.close(); } catch { /* best-effort */ }
    _db = null;
  }
}

function requireDb(): Sqlite3DB {
  if (!_db) throw new Error("sqlite-wasm engine not initialized — call initDatabase() first");
  return _db;
}
