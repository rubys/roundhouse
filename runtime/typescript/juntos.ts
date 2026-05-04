// Roundhouse TypeScript primitive-runtime layer.
//
// Target-mechanism only — bridges node primitives (better-sqlite3,
// Action Cable WebSocket) to the 12-method `ActiveRecordAdapter`
// contract that transpiled framework Ruby calls into. Mirrors
// spinel's `runtime/spinel/` (sqlite_adapter, in_memory_adapter,
// broadcasts) — same role, target-specific mechanism.
//
// Anything with a Ruby analog in `runtime/ruby/` lives there and
// reaches TS via transpile, not here. Earlier revisions of this
// file carried a parallel framework runtime (ApplicationRecord,
// ErrorCollection, CollectionProxy, an imperative Router) that
// shadowed `runtime/ruby/active_record/`, validations, and
// `runtime/ruby/action_dispatch/router.rb`; those are gone now.
// Routing in particular is the transpiled `Router.match(method,
// path, table)` (see `runtime/ruby/action_dispatch/router.rb`),
// keyed on the runtime-loader `extra_roots` mechanism.

import Database from "better-sqlite3";

import { ActiveRecord } from "./active_record_base.js";

// ── DB connection lifecycle ──

let _db: Database.Database | null = null;

/** Install an already-opened database in the module-level slot AND
 *  point the framework's `ActiveRecord.adapter` at a SQLite-backed
 *  adapter wrapping it. Production path: the server opens a file-
 *  backed DB and calls this; subsequent `Model.find/all/where/...`
 *  calls in transpiled framework Ruby resolve through the adapter. */
export function installDb(db: Database.Database): void {
  if (_db && _db !== db) {
    try { _db.close(); } catch { /* best-effort */ }
  }
  _db = db;
  ActiveRecord.adapter = new SqliteActiveRecordAdapter(db);
}

/** Open a fresh :memory: SQLite connection, run the schema DDL, and
 *  install it in the module-level slot. Called from `Fixtures.setup`
 *  at the top of every spec. Production callers open their own file-
 *  backed connection and use `installDb` instead. */
export function setupTestDb(schema_sql: string): void {
  const db = new Database(":memory:");
  db.exec(schema_sql);
  installDb(db);
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


// ── ActiveRecord adapter shim ──
//
// The framework Ruby (transpiled from `runtime/ruby/active_record/`)
// calls into a stable 12-method API surface for all DB access. Each
// target language provides an implementation of this interface; that's
// the per-target glue. The framework Ruby is portable across targets
// because it touches nothing else.

/** A single row as plain primitives. Adapters serialize to/from this
 *  shape; the per-model classes typecast on the way out. */
export type Row = Record<string, string | number | null>;

/** Equality conditions for `where(table, conditions)`. Richer queries
 *  (range, comparison, joins) live above the adapter. */
export type Conditions = Record<string, string | number | null>;

export type ForeignKey = { column: string; references: string };
export interface AdapterSchema {
  columns: string[];
  foreign_keys: ForeignKey[];
}

/** The full adapter surface — twelve methods. Framework Ruby calls
 *  exclusively through this interface; targets implement it. */
export interface ActiveRecordAdapter {
  // DDL
  create_table(name: string, columns: string[], foreign_keys?: ForeignKey[]): void;
  drop_table(name: string): void;
  schema(table: string): AdapterSchema | null;
  // Read
  find(table: string, id: number): Row | null;
  all(table: string): Row[];
  where(table: string, conditions: Conditions): Row[];
  count(table: string): number;
  exists(table: string, id: number): boolean;
  // Write
  insert(table: string, row: Row): number;
  update(table: string, id: number, row: Row): boolean;
  delete(table: string, id: number): boolean;
}

/** In-memory test adapter. Mirrors the semantics of
 *  `runtime/ruby/active_record/in_memory_adapter.rb`; transpiling that
 *  file to this shape is a milestone validation point for the
 *  strategic bet (currently hand-written here while the body-walker
 *  catches up to the patterns it uses). */
export class InMemoryActiveRecordAdapter implements ActiveRecordAdapter {
  private tables: Map<string, Map<number, Row>> = new Map();
  private schemas: Map<string, AdapterSchema> = new Map();
  private nextId: Map<string, number> = new Map();

  create_table(name: string, columns: string[], foreign_keys: ForeignKey[] = []): void {
    this.tables.set(name, new Map());
    this.schemas.set(name, { columns, foreign_keys });
  }

  drop_table(name: string): void {
    this.tables.delete(name);
    this.schemas.delete(name);
    this.nextId.delete(name);
  }

  schema(table: string): AdapterSchema | null {
    return this.schemas.get(table) ?? null;
  }

  insert(table: string, row: Row): number {
    const t = this.tables.get(table);
    if (!t) throw new Error(`insert: unknown table ${table}`);
    const id = (this.nextId.get(table) ?? 0) + 1;
    this.nextId.set(table, id);
    const stored = { ...row, id };
    t.set(id, stored);
    return id;
  }

  update(table: string, id: number, row: Row): boolean {
    const t = this.tables.get(table);
    if (!t || !t.has(id)) return false;
    t.set(id, { ...row, id });
    return true;
  }

  delete(table: string, id: number): boolean {
    const t = this.tables.get(table);
    if (!t) return false;
    return t.delete(id);
  }

  find(table: string, id: number): Row | null {
    return this.tables.get(table)?.get(id) ?? null;
  }

  all(table: string): Row[] {
    const t = this.tables.get(table);
    return t ? Array.from(t.values()) : [];
  }

  where(table: string, conditions: Conditions): Row[] {
    const entries = Object.entries(conditions);
    return this.all(table).filter((row) =>
      entries.every(([k, v]) => row[k] === v),
    );
  }

  count(table: string): number {
    return this.tables.get(table)?.size ?? 0;
  }

  exists(table: string, id: number): boolean {
    return this.tables.get(table)?.has(id) ?? false;
  }
}

/** better-sqlite3-backed adapter — production path. Wraps a
 *  pre-opened Database connection (the server opens it, runs the
 *  schema DDL, and hands it here via `installDb`). All queries are
 *  prepared on each call; better-sqlite3 caches plans internally so
 *  the cost is negligible for the scaffold-blog query mix. */
export class SqliteActiveRecordAdapter implements ActiveRecordAdapter {
  constructor(private readonly db: Database.Database) {}

  create_table(name: string, columns: string[], _foreign_keys: ForeignKey[] = []): void {
    // CREATE TABLE generation is handled by the schema DDL the
    // server applies before installDb. The transpiled framework's
    // `Schema.create!` path doesn't call `create_table` for the
    // SQLite adapter; this implementation exists to satisfy the
    // interface and is deliberately a no-op.
    void name; void columns;
  }

  drop_table(name: string): void {
    this.db.exec(`DROP TABLE IF EXISTS ${name}`);
  }

  schema(table: string): AdapterSchema | null {
    const rows = this.db
      .prepare(`SELECT name FROM pragma_table_info(?)`)
      .all(table) as Array<{ name: string }>;
    if (rows.length === 0) return null;
    return { columns: rows.map((r) => r.name), foreign_keys: [] };
  }

  find(table: string, id: number): Row | null {
    const row = this.db
      .prepare(`SELECT * FROM ${table} WHERE id = ?`)
      .get(id) as Row | undefined;
    return row ?? null;
  }

  all(table: string): Row[] {
    return this.db.prepare(`SELECT * FROM ${table}`).all() as Row[];
  }

  where(table: string, conditions: Conditions): Row[] {
    const entries = Object.entries(conditions);
    if (entries.length === 0) return this.all(table);
    const clause = entries.map(([k]) => `${k} = ?`).join(" AND ");
    const values = entries.map(([, v]) => v);
    return this.db
      .prepare(`SELECT * FROM ${table} WHERE ${clause}`)
      .all(...values) as Row[];
  }

  count(table: string): number {
    const row = this.db
      .prepare(`SELECT COUNT(*) AS c FROM ${table}`)
      .get() as { c: number };
    return row.c;
  }

  exists(table: string, id: number): boolean {
    return this.find(table, id) !== null;
  }

  insert(table: string, row: Row): number {
    const cols = Object.keys(row);
    const placeholders = cols.map(() => "?").join(", ");
    const values = cols.map((c) => row[c]);
    if (cols.length === 0) {
      const info = this.db
        .prepare(`INSERT INTO ${table} DEFAULT VALUES`)
        .run();
      return Number(info.lastInsertRowid);
    }
    const info = this.db
      .prepare(`INSERT INTO ${table} (${cols.join(", ")}) VALUES (${placeholders})`)
      .run(...values);
    return Number(info.lastInsertRowid);
  }

  update(table: string, id: number, row: Row): boolean {
    const cols = Object.keys(row).filter((c) => c !== "id");
    if (cols.length === 0) return true;
    const sets = cols.map((c) => `${c} = ?`).join(", ");
    const values = cols.map((c) => row[c]);
    const info = this.db
      .prepare(`UPDATE ${table} SET ${sets} WHERE id = ?`)
      .run(...values, id);
    return info.changes > 0;
  }

  delete(table: string, id: number): boolean {
    const info = this.db
      .prepare(`DELETE FROM ${table} WHERE id = ?`)
      .run(id);
    return info.changes > 0;
  }
}

// Controller/router surface — controllers return ActionResponse;
// the router's match table lets tests dispatch without a live HTTP
// server (pure in-process function calls).

/** Every controller action returns one of these. Fields are
 *  optional so actions can pick the shape they need:
 *    - `body`: the HTML string the view rendered (for GET actions)
 *    - `status`: HTTP status code (default 200; 422 for
 *      unprocessable, 302 for redirects)
 *    - `location`: redirect target URL; test assertions on
 *      `assert_redirected_to` check this field. */
export type ActionResponse = {
  body?: string;
  status?: number;
  location?: string;
};

