// Roundhouse TypeScript worker runtime — dedicated database Worker entry.
//
// Loaded into a dedicated `Worker` (not the SharedWorker — see
// `rails.ts`'s `spawnDbWorker` for the spawn site). The dedicated
// Worker is the only browser context where SQLite WASM can use the
// `FileSystemSyncAccessHandle` API for synchronous OPFS I/O, so
// here is the *only* place `sqlite_wasm_engine` runs.
//
// Protocol: receives `{ id, type, ...payload }` messages, replies with
// `{ id, type: 'result' | 'error', ... }`. The SharedWorker side
// (`active_record_worker.ts`) keeps a `Map<id, resolver>` and
// correlates responses to the awaited Promises.
//
// Message types:
//   - `init` → open the DB. Reply: `{ type: 'ready' }` (no `id`).
//   - `exec` → run a single SQL with positional `params`. Reply:
//             `{ type: 'result', rows, changes, lastInsertRowId }`.
//   - `execSQL` → run a multi-statement SQL blob (schema/migrations).
//                 Reply: `{ type: 'result' }`.
//   - `begin` / `commit` / `rollback` → transaction control. Reply:
//             `{ type: 'result' }`.
//
// Active Storage file ops (juntos's `file:upload` / `file:download` /
// etc.) are intentionally not implemented — Active Storage is deferred
// in the initial worker target plan.

import { initDatabase, exec, execSQL, type InitOptions } from "./sqlite_wasm_engine.js";

// ── Message protocol types ──

interface InitMessage {
  type: "init";
  config?: InitOptions;
}

interface ExecMessage {
  type: "exec";
  id: string;
  sql: string;
  params?: unknown[];
}

interface ExecSqlMessage {
  type: "execSQL";
  id: string;
  sql: string;
}

interface TxMessage {
  type: "begin" | "commit" | "rollback";
  id: string;
}

type IncomingMessage = InitMessage | ExecMessage | ExecSqlMessage | TxMessage;

// ── Dispatch ──

declare const self: DedicatedWorkerGlobalScope;

self.onmessage = async ({ data }: MessageEvent<IncomingMessage>) => {
  if (data.type === "init") {
    try {
      await initDatabase(data.config ?? {});
      self.postMessage({ type: "ready" });
    } catch (e) {
      self.postMessage({ type: "error", error: errorMessage(e) });
    }
    return;
  }

  if (data.type === "exec") {
    const { id, sql, params } = data;
    try {
      const result = exec(sql, params ?? []);
      self.postMessage({ id, type: "result", ...result });
    } catch (e) {
      self.postMessage({ id, type: "error", error: errorMessage(e) });
    }
    return;
  }

  if (data.type === "execSQL") {
    const { id, sql } = data;
    try {
      execSQL(sql);
      self.postMessage({
        id,
        type: "result",
        rows: [],
        changes: 0,
        lastInsertRowId: null,
      });
    } catch (e) {
      self.postMessage({ id, type: "error", error: errorMessage(e) });
    }
    return;
  }

  if (data.type === "begin" || data.type === "commit" || data.type === "rollback") {
    const { id, type } = data;
    const sql = type.toUpperCase();
    try {
      exec(sql, []);
      self.postMessage({
        id,
        type: "result",
        rows: [],
        changes: 0,
        lastInsertRowId: null,
      });
    } catch (e) {
      self.postMessage({ id, type: "error", error: errorMessage(e) });
    }
    return;
  }
};

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
