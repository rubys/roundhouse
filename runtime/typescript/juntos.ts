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

  static find<T extends typeof ApplicationRecord>(this: T, id: number): InstanceType<T> | null {
    const row = conn()
      .prepare(`SELECT * FROM ${(this as any).table_name} WHERE id = ?`)
      .get(id);
    if (!row) return null;
    return Object.assign(new (this as any)(), row) as InstanceType<T>;
  }

  /** `Model.all()` — every row, loaded as instances. */
  static all<T extends typeof ApplicationRecord>(this: T): InstanceType<T>[] {
    const rows = conn()
      .prepare(`SELECT * FROM ${(this as any).table_name}`)
      .all();
    return rows.map((row) => Object.assign(new (this as any)(), row)) as InstanceType<T>[];
  }

  /** `Model.last` — highest-id row, or null when the table's empty. */
  static last<T extends typeof ApplicationRecord>(this: T): InstanceType<T> | null {
    const row = conn()
      .prepare(`SELECT * FROM ${(this as any).table_name} ORDER BY id DESC LIMIT 1`)
      .get();
    if (!row) return null;
    return Object.assign(new (this as any)(), row) as InstanceType<T>;
  }

  static new(_attrs?: any): any {
    return new (this as any)();
  }

  /** `@record.reload` — re-fetch by id and copy over self. */
  reload(): void {
    const fresh = (this.constructor as any).find((this as any).id);
    if (fresh) Object.assign(this, fresh);
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
  get length(): number {
    return 0;
  }
  each(_fn: (item: T) => void): void {}
  map<U>(_fn: (item: T) => U): U[] {
    return [];
  }
  forEach(_fn: (item: T) => void): void {}
}

export const modelRegistry: Record<string, any> = {};

// ActiveRecord appears in extends clauses after normalization
// (`ActiveRecord::Base` → `ActiveRecord`).
export class ActiveRecord extends ApplicationRecord {}

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

/** Context passed to every action. `params` merges path params +
 *  form body. `request` / `session` are placeholders for future
 *  work; tests never populate them. */
export type ActionContext = {
  params: Record<string, any>;
  request?: any;
  session?: Record<string, any>;
};

/** One entry in the router's match table. */
export type Route = {
  method: string;
  path: string;
  handler: (ctx: ActionContext) => Promise<ActionResponse> | ActionResponse;
};

export class Router {
  private static routes: Route[] = [];

  /** Clear the table — used in tests between runs to avoid
   *  cross-test leakage. Generated code calls the builders
   *  idempotently at import time; repeated imports accumulate. */
  static reset(): void {
    Router.routes = [];
  }

  /** Mount a path to a controller action. Takes 2 or 3 args — the
   *  two-arg form is `Router.root(Controller, "action")`; the
   *  three-arg form is `Router.root(path, Controller, "action")`
   *  which matches the current emitter shape. Either way the path
   *  defaults to `/`. */
  static root(
    a: string | any,
    b?: any,
    c?: string,
  ): void {
    const [path, controller, action]: [string, any, string] =
      typeof a === "string" ? [a, b, c ?? "index"] : ["/", a, b];
    const handler_name = action === "new" ? "$new" : action;
    const handler = controller[handler_name];
    if (!handler) return;
    Router.routes.push({ method: "GET", path, handler });
  }

  /** Mount a resource's seven standard actions. Options:
   *    - `only`: restrict to listed actions
   *    - `except`: exclude listed actions
   *    - `nested`: array of nested resource specs (each with
   *      `name`, `controller`, and optional `only` / `except`)
   *
   *  Rails' `resources :articles do resources :comments end` lowers
   *  to a call with `nested: [{ name: "comments", controller:
   *  CommentsController, only: ["create", "destroy"] }]`. */
  static resources(name: string, controller: any, opts?: { only?: string[]; except?: string[]; nested?: Array<{ name: string; controller: any; only?: string[]; except?: string[] }> }): void {
    Router.addResourceRoutes(name, controller, opts, null);
    if (opts?.nested) {
      for (const nested of opts.nested) {
        const parent_singular = singularize(name);
        Router.addResourceRoutes(nested.name, nested.controller, nested, {
          parent_plural: name,
          parent_singular,
        });
      }
    }
  }

  private static addResourceRoutes(
    name: string,
    controller: any,
    opts: { only?: string[]; except?: string[] } | undefined,
    scope: { parent_plural: string; parent_singular: string } | null,
  ): void {
    const standard: Array<[string, string, string]> = [
      ["index", "GET", ""],
      ["new", "GET", "/new"],
      ["create", "POST", ""],
      ["show", "GET", "/:id"],
      ["edit", "GET", "/:id/edit"],
      ["update", "PATCH", "/:id"],
      ["destroy", "DELETE", "/:id"],
    ];
    for (const [action, method, suffix] of standard) {
      if (opts?.only && !opts.only.includes(action)) continue;
      if (opts?.except && opts.except.includes(action)) continue;
      const base = scope
        ? `/${scope.parent_plural}/:${scope.parent_singular}_id/${name}`
        : `/${name}`;
      const path = `${base}${suffix}`;
      const handler_name = action === "new" ? "$new" : action;
      const handler = controller[handler_name];
      if (!handler) continue;
      Router.routes.push({ method, path, handler });
    }
  }

  static get(path: string, controller: any, action: string): void {
    const handler = controller[action === "new" ? "$new" : action];
    if (handler) Router.routes.push({ method: "GET", path, handler });
  }
  static post(path: string, controller: any, action: string): void {
    const handler = controller[action === "new" ? "$new" : action];
    if (handler) Router.routes.push({ method: "POST", path, handler });
  }
  static put(path: string, controller: any, action: string): void {
    const handler = controller[action === "new" ? "$new" : action];
    if (handler) Router.routes.push({ method: "PUT", path, handler });
  }
  static patch(path: string, controller: any, action: string): void {
    const handler = controller[action === "new" ? "$new" : action];
    if (handler) Router.routes.push({ method: "PATCH", path, handler });
  }
  static delete(path: string, controller: any, action: string): void {
    const handler = controller[action === "new" ? "$new" : action];
    if (handler) Router.routes.push({ method: "DELETE", path, handler });
  }

  /** Match a request to a route. Returns the handler plus a merged
   *  params record (path segments extracted from the URL). Used by
   *  the test client to dispatch without spinning up an HTTP
   *  server. */
  static match(method: string, path: string): { handler: Route["handler"]; params: Record<string, string> } | null {
    for (const route of Router.routes) {
      if (route.method !== method) continue;
      const match = Router.tryMatchPath(route.path, path);
      if (match) return { handler: route.handler, params: match };
    }
    return null;
  }

  private static tryMatchPath(pattern: string, path: string): Record<string, string> | null {
    const pat_parts = pattern.split("/").filter(Boolean);
    const path_parts = path.split("/").filter(Boolean);
    if (pat_parts.length !== path_parts.length) return null;
    const params: Record<string, string> = {};
    for (let i = 0; i < pat_parts.length; i++) {
      const p = pat_parts[i];
      const v = path_parts[i];
      if (p.startsWith(":")) {
        params[p.slice(1)] = v;
      } else if (p !== v) {
        return null;
      }
    }
    return params;
  }
}

/** Minimal English singularizer for router-internal use. Matches
 *  the patterns the scaffold blog exercises (`articles` →
 *  `article`, `comments` → `comment`). A fuller inflector lives in
 *  the generator; this is enough for the runtime path. */
function singularize(plural: string): string {
  if (plural.endsWith("ies")) return plural.slice(0, -3) + "y";
  if (plural.endsWith("ses")) return plural.slice(0, -2);
  if (plural.endsWith("s")) return plural.slice(0, -1);
  return plural;
}
