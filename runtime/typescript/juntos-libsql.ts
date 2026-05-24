// Roundhouse TypeScript primitive-runtime layer — libsql variant.
//
// Mirrors `juntos.ts` but bridges to `@libsql/client` (Turso's
// async-native fork of SQLite) instead of better-sqlite3. The two
// files share the public surface — `installDb`, `setupTestDb`,
// `setBroadcaster`, `Row`, `Conditions`, `AdapterSchema`,
// `ForeignKey`, `ActiveRecordAdapter`, the controller/router types,
// and `ActionResponse` — so the transpiled framework code
// (Base.find/all/where/...) doesn't care which one the emit pipeline
// picked.
//
// The single semantic difference: every adapter method here returns
// a Promise. The transpiled framework Ruby is async-colored under
// the libsql profile, so `await adapter.find(...)` is correct in
// the runtime; under the better-sqlite3 profile the same site emits
// `await adapter.find(...)` against the sync adapter, where `await`
// of a non-Promise is the identity. Both paths produce correct code
// — the colorer just gates which adapter the runtime selects.
//
// `setupTestDb` is async here because libsql's `execute` is async;
// callers in transpiled `Fixtures.setup` need to `await` it.

import { type Client, type InValue } from "@libsql/client";

import { ActiveRecord } from "./active_record_base.js";

// ── DB connection lifecycle ──

let _client: Client | null = null;

/** Install an already-opened libsql Client in the module-level slot
 *  AND point the framework's `ActiveRecord.adapter` at a libsql-
 *  backed adapter wrapping it. Production path: the server opens a
 *  file-backed (or remote / replicated) Client and calls this;
 *  subsequent `Model.find/all/where/...` calls in transpiled
 *  framework Ruby resolve through the adapter. */
export function installDb(client: Client): void {
  if (_client && _client !== client) {
    try { _client.close(); } catch { /* best-effort */ }
  }
  _client = client;
  ActiveRecord.adapter = new LibsqlActiveRecordAdapter(client);
}

/** Open a fresh in-memory libsql Client, run the schema DDL, and
 *  install it. Called from `Fixtures.setup` at the top of every
 *  spec. Async because libsql's `execute` is async — all callers
 *  must `await`. */
export async function setupTestDb(schema_sql: string): Promise<void> {
  const { createClient } = await import("@libsql/client");
  const client = createClient({ url: ":memory:" });
  // Schema arrives as one blob containing many `;`-terminated
  // statements. libsql doesn't support multi-statement execute,
  // so split on semicolons (skipping empty fragments).
  for (const stmt of schema_sql.split(";")) {
    const trimmed = stmt.trim();
    if (trimmed) await client.execute(trimmed);
  }
  installDb(client);
  // Adopt the same client for the Level-3 Db primitive surface so
  // lowerer-emitted `_adapter_*` methods (Phase 1 Arel-rewritten
  // SELECTs) see the same rows as the legacy juntos AR adapter.
  // Lazy import keeps the libsql variant out of sync-profile bundles.
  const { Db } = await import("./db.js");
  Db.install(client);
}

/** Signature for the server-side broadcaster. The server installs
 *  one via `setBroadcaster` when it's ready to forward fragments to
 *  subscribed Action Cable clients. Test mode leaves it null so
 *  broadcasts become no-ops. */
export type Broadcaster = (stream: string, html: string) => void;

let broadcaster: Broadcaster | null = null;

/** Install the broadcaster. Called by the HTTP server's cable
 *  handler once the WebSocket is ready to forward fragments. */
export function setBroadcaster(fn: Broadcaster | null): void {
  broadcaster = fn;
}

export function broadcast(stream: string, html: string): void {
  broadcaster?.(stream, html);
}

// ── ActiveRecord adapter shim ──
//
// Same 12-method surface as juntos.ts, but every method returns a
// Promise here — libsql's `execute` is async-native. The
// `ActiveRecordAdapter` interface widens its return types to allow
// either Promise<T> or T directly so a single declaration covers
// both the libsql (async) and better-sqlite3 (sync) implementations.

/** A single row as plain primitives. */
export type Row = Record<string, string | number | null>;

/** Equality conditions for `where(table, conditions)`. */
export type Conditions = Record<string, string | number | null>;

export type ForeignKey = { column: string; references: string };
export interface AdapterSchema {
  columns: string[];
  foreign_keys: ForeignKey[];
}

/** Alias for the transpiled-framework type name. `runtime/ruby/
 *  active_record/base.rbs` types `ActiveRecord.adapter` as
 *  `AdapterInterface`; each target maps that name onto its own
 *  concrete adapter type. libsql variant points it at the
 *  ActiveRecordAdapter interface declared just below. */
export type AdapterInterface = ActiveRecordAdapter;

/** The full adapter surface. Return types are `T | Promise<T>` so
 *  both the libsql (async) and better-sqlite3 (sync) adapters
 *  satisfy the same interface. Callers always `await` so the
 *  Promise-or-not distinction is transparent. */
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

/** libsql-backed adapter. Constructor takes a pre-opened Client
 *  (the server opens it, applies schema DDL, hands it here). Every
 *  method awaits `client.execute`. Prepared-statement caching is
 *  internal to libsql; we don't repeat it. */
export class LibsqlActiveRecordAdapter implements ActiveRecordAdapter {
  constructor(private readonly client: Client) {}

  async create_table(name: string, columns: string[], _foreign_keys: ForeignKey[] = []): Promise<void> {
    // CREATE TABLE generation is handled by the schema DDL the
    // server applies before installDb. The transpiled framework's
    // `Schema.create!` path doesn't call `create_table` for the
    // libsql adapter; this implementation exists to satisfy the
    // interface and is deliberately a no-op.
    void name; void columns;
  }

  async drop_table(name: string): Promise<void> {
    await this.client.execute(`DROP TABLE IF EXISTS ${name}`);
  }

  async schema(table: string): Promise<AdapterSchema | null> {
    const result = await this.client.execute({
      sql: `SELECT name FROM pragma_table_info(?)`,
      args: [table],
    });
    if (result.rows.length === 0) return null;
    return {
      columns: result.rows.map((r) => String((r as Record<string, unknown>).name)),
      foreign_keys: [],
    };
  }

  async find(table: string, id: number): Promise<Row | null> {
    const result = await this.client.execute({
      sql: `SELECT * FROM ${table} WHERE id = ?`,
      args: [id],
    });
    return (result.rows[0] as Row | undefined) ?? null;
  }

  async all(table: string): Promise<Row[]> {
    const result = await this.client.execute(`SELECT * FROM ${table}`);
    return result.rows as unknown as Row[];
  }

  async where(table: string, conditions: Conditions): Promise<Row[]> {
    const entries = Object.entries(conditions);
    if (entries.length === 0) return this.all(table);
    const clause = entries.map(([k]) => `${k} = ?`).join(" AND ");
    const values = entries.map(([, v]) => v as InValue);
    const result = await this.client.execute({
      sql: `SELECT * FROM ${table} WHERE ${clause}`,
      args: values,
    });
    return result.rows as unknown as Row[];
  }

  async count(table: string): Promise<number> {
    const result = await this.client.execute(`SELECT COUNT(*) AS c FROM ${table}`);
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
      const result = await this.client.execute(`INSERT INTO ${table} DEFAULT VALUES`);
      return Number(result.lastInsertRowid ?? 0n);
    }
    const placeholders = cols.map(() => "?").join(", ");
    const values = cols.map((c) => row[c] as InValue);
    const result = await this.client.execute({
      sql: `INSERT INTO ${table} (${cols.join(", ")}) VALUES (${placeholders})`,
      args: values,
    });
    return Number(result.lastInsertRowid ?? 0n);
  }

  async update(table: string, id: number, row: Row): Promise<boolean> {
    const cols = Object.keys(row).filter((c) => c !== "id");
    if (cols.length === 0) return true;
    const sets = cols.map((c) => `${c} = ?`).join(", ");
    const values = cols.map((c) => row[c] as InValue);
    const result = await this.client.execute({
      sql: `UPDATE ${table} SET ${sets} WHERE id = ?`,
      args: [...values, id],
    });
    return result.rowsAffected > 0;
  }

  async delete(table: string, id: number): Promise<boolean> {
    const result = await this.client.execute({
      sql: `DELETE FROM ${table} WHERE id = ?`,
      args: [id],
    });
    return result.rowsAffected > 0;
  }
}

// Historical note: an InMemoryActiveRecordAdapter mirror previously
// lived here as the libsql-profile fallback for base_test. Removed in
// tandem with the per-target adapter cleanup; a follow-on session will
// re-enable base_test wired against a real libsql connection.

// ── Controller/router surface ──
//
// Re-exported from juntos.ts unchanged — these types are
// target-mechanism-agnostic. Kept here so `server-libsql.ts` can
// import everything from one place without pulling in the sqlite
// runtime alongside.

export type ActionResponse = {
  body?: string;
  status?: number;
  location?: string;
  content_type?: string;
};
