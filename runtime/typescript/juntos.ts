// Roundhouse TypeScript runtime — minimal Juntos-shape stub.
//
// The TS emitter targets Juntos (https://www.ruby2js.com/docs/juntos/).
// This stub provides the subset the emitted project needs for Phase 3:
// typed model surface, validation primitives, and a better-sqlite3-backed
// persistence layer keyed on per-subclass metadata (table_name, columns,
// belongsToChecks, dependentChildren). Real Juntos takes over in
// production via tsconfig path mapping.

import Database from "better-sqlite3";

// ── DB connection lifecycle ──

let _db: Database.Database | null = null;

/** Open a fresh :memory: SQLite connection, run the schema DDL, and
 *  install it in the module-level slot. Called from `Fixtures.setup`
 *  at the top of every spec. */
export function setupTestDb(schema_sql: string): void {
  if (_db) _db.close();
  _db = new Database(":memory:");
  _db.exec(schema_sql);
}

/** Borrow the current connection. Throws if `setupTestDb` hasn't
 *  been called yet. */
export function conn(): Database.Database {
  if (!_db) {
    throw new Error("test db not initialized; call setupTestDb first");
  }
  return _db;
}

// ── Model surface ──

/** A `{ fk, targetName }` pair the generated model declares as static
 *  metadata so `save` can check that `belongs_to` references resolve.
 *  `targetName` is a class name looked up in `modelRegistry` at
 *  runtime — avoids circular import initialization pitfalls. */
export interface BelongsToCheck {
  fk: string;
  targetName: string;
}

/** `{ fk, targetName }` pair for a `has_many dependent: :destroy`
 *  relationship — `destroy` uses it to cascade. */
export interface DependentChild {
  fk: string;
  targetName: string;
}

export class ApplicationRecord {
  // Metadata subclasses populate. Defaults keep the base class usable
  // on its own in tests that only exercise validation.
  static table_name: string = "";
  static columns: string[] = [];
  static belongsToChecks: BelongsToCheck[] = [];
  static dependentChildren: DependentChild[] = [];

  id: number = 0;
  errors: ErrorCollection = new ErrorCollection();

  /** Rails-semantics `save`: runs validations (and belongs_to
   *  existence checks) first; on success, INSERTs when `id === 0`
   *  otherwise UPDATEs. Exposed as a getter to match Juntos's
   *  property-style API. */
  get save(): boolean {
    this.errors = new ErrorCollection();
    this.validate();
    if (!this.errors.none) return false;

    const cls = this.constructor as typeof ApplicationRecord;

    for (const { fk, targetName } of cls.belongsToChecks) {
      const fkVal = (this as any)[fk];
      const target = modelRegistry[targetName] as typeof ApplicationRecord;
      if (fkVal === 0 || fkVal == null || target.find(fkVal) === null) {
        return false;
      }
    }

    const db = conn();
    const cols = cls.columns;
    const placeholders = cols.map(() => "?").join(", ");
    const values = cols.map((c) => (this as any)[c]);

    if (this.id === 0) {
      const sql = `INSERT INTO ${cls.table_name} (${cols.join(", ")}) VALUES (${placeholders})`;
      const info = db.prepare(sql).run(...values);
      this.id = Number(info.lastInsertRowid);
    } else {
      const sets = cols.map((c) => `${c} = ?`).join(", ");
      const sql = `UPDATE ${cls.table_name} SET ${sets} WHERE id = ?`;
      db.prepare(sql).run(...values, this.id);
    }
    return true;
  }

  /** Cascade `dependent: :destroy` children first (so each child's
   *  own destroy logic runs), then remove this row. */
  get destroy(): this {
    const cls = this.constructor as typeof ApplicationRecord;
    const db = conn();

    for (const { fk, targetName } of cls.dependentChildren) {
      const target = modelRegistry[targetName] as typeof ApplicationRecord;
      const rows = db
        .prepare(
          `SELECT * FROM ${target.table_name} WHERE ${fk} = ?`,
        )
        .all(this.id) as Record<string, any>[];
      for (const row of rows) {
        const child = Object.assign(new (target as any)(), row);
        child.destroy;
      }
    }

    db.prepare(`DELETE FROM ${cls.table_name} WHERE id = ?`).run(this.id);
    return this;
  }

  static count(): number {
    const row = conn()
      .prepare(`SELECT COUNT(*) AS c FROM ${this.table_name}`)
      .get() as { c: number };
    return row.c;
  }

  static find<T extends ApplicationRecord>(id: number): T | null {
    const row = conn()
      .prepare(`SELECT * FROM ${this.table_name} WHERE id = ?`)
      .get(id);
    if (!row) return null;
    return Object.assign(new (this as any)(), row) as T;
  }

  static new(_attrs?: any): any {
    return new (this as any)();
  }

  // Broadcast callback registration — emitter renders `broadcasts_to`
  // declarations as post-class-body calls to these. Stub dispatches
  // nothing; real Juntos pushes to Turbo Stream channels.
  static afterSave(_fn: (record: any) => any): void {}
  static afterDestroy(_fn: (record: any) => any): void {}
  static afterCreate(_fn: (record: any) => any): void {}
  static afterUpdate(_fn: (record: any) => any): void {}
  static afterCommit(_fn: (record: any) => any): void {}

  validate(): void {}

  validates_presence_of(field: string): void {
    const value = (this as any)[field];
    if (value === null || value === undefined || value === "") {
      this.errors.add(field, "can't be blank");
    }
  }

  validates_length_of(
    field: string,
    opts: { minimum?: number; maximum?: number } = {},
  ): void {
    const value = (this as any)[field];
    const len = value == null ? 0 : (value as { length: number }).length ?? 0;
    if (opts.minimum !== undefined && len < opts.minimum) {
      this.errors.add(
        field,
        `is too short (minimum is ${opts.minimum} characters)`,
      );
    }
    if (opts.maximum !== undefined && len > opts.maximum) {
      this.errors.add(
        field,
        `is too long (maximum is ${opts.maximum} characters)`,
      );
    }
  }

  // Turbo-stream broadcasts on the record. Real implementations push
  // HTML to subscribed WebSocket channels; the stub is a no-op.
  broadcastPrependTo(_stream: string): void {}
  broadcastAppendTo(_stream: string): void {}
  broadcastReplaceTo(_stream: string): void {}
  broadcastRemoveTo(_stream: string): void {}
  broadcastUpdateTo(_stream: string): void {}

  // Escape hatch for runtime-materialized column accessors.
  [key: string]: any;
}

export class ErrorCollection {
  private _errors: Array<{ field: string; message: string }> = [];

  get none(): boolean {
    return this._errors.length === 0;
  }

  get any(): boolean {
    return this._errors.length > 0;
  }

  get count(): number {
    return this._errors.length;
  }

  add(field: string, message: string): void {
    this._errors.push({ field, message });
  }
}

export class Reference<T = any> {
  constructor(_cls: any, _id: any) {}
  get value(): T {
    return null as unknown as T;
  }
}

export class CollectionProxy<T = any> {
  constructor(_owner: any, _config: any, _target: any) {}
  get all(): T[] {
    return [];
  }
  get size(): number {
    return 0;
  }
  get any(): boolean {
    return false;
  }
  each(_fn: (item: T) => void): void {}
}

export const modelRegistry: Record<string, any> = {};

// ActiveRecord appears in extends clauses after normalization
// (`ActiveRecord::Base` → `ActiveRecord`).
export class ActiveRecord extends ApplicationRecord {}

// Controller/router surface — stubs so generated controller code can
// import and reference these names without tsc erroring.
export type ActionContext = {
  params: Record<string, any>;
  request: any;
  session: Record<string, any>;
};

export class Router {
  static root(_target: string): void {}
  static resources(_name: string, _opts?: any, _nested?: () => void): void {}
  static get(_path: string, _controller: any, _action: string): void {}
  static post(_path: string, _controller: any, _action: string): void {}
  static put(_path: string, _controller: any, _action: string): void {}
  static patch(_path: string, _controller: any, _action: string): void {}
  static delete(_path: string, _controller: any, _action: string): void {}
}
