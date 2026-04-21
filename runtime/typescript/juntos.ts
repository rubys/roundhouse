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

/** Install an already-opened database in the module-level slot.
 *  Production path: the server opens a file-backed DB and calls
 *  this. Subsequent `conn()` calls return the installed db. */
export function installDb(db: Database.Database): void {
  if (_db && _db !== db) {
    try { _db.close(); } catch { /* best-effort */ }
  }
  _db = db;
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

/** Borrow the current connection. Throws if no database has been
 *  installed — tests call `setupTestDb`; the server calls
 *  `installDb` after opening its file-backed DB. */
export function conn(): Database.Database {
  if (!_db) {
    throw new Error("db not initialized; call setupTestDb or installDb first");
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

export class ApplicationRecord {
  // Metadata subclasses populate. Defaults keep the base class usable
  // on its own in tests that only exercise validation.
  static table_name: string = "";
  static columns: string[] = [];
  static belongsToChecks: BelongsToCheck[] = [];
  static dependentChildren: DependentChild[] = [];
  /** Renders this record as a partial HTML fragment. Set by the
   *  emitter after loading the model's `_record.html.ts` partial:
   *  `Model.renderPartial = render`. Broadcasts use this to produce
   *  the `<template>` contents for append/prepend/replace actions. */
  static renderPartial: ((record: any) => string) | null = null;

  id: number = 0;
  errors: ErrorCollection = new ErrorCollection();

  /** Rails-semantics `save`: runs validations (and belongs_to
   *  existence checks) first; on success, INSERTs when `id === 0`
   *  otherwise UPDATEs. Fires afterCreate (for new records) or
   *  afterUpdate (for existing), then afterSave + afterCommit.
   *  Exposed as a getter to match Juntos's property-style API. */
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

    const isNewRecord = this.id === 0;
    if (isNewRecord) {
      const sql = `INSERT INTO ${cls.table_name} (${cols.join(", ")}) VALUES (${placeholders})`;
      const info = db.prepare(sql).run(...values);
      this.id = Number(info.lastInsertRowid);
    } else {
      const sets = cols.map((c) => `${c} = ?`).join(", ");
      const sql = `UPDATE ${cls.table_name} SET ${sets} WHERE id = ?`;
      db.prepare(sql).run(...values, this.id);
    }
    if (isNewRecord) {
      this._fireCallbacks("_afterCreateCallbacks");
    } else {
      this._fireCallbacks("_afterUpdateCallbacks");
    }
    this._fireCallbacks("_afterSaveCallbacks");
    this._fireCallbacks("_afterCommitCallbacks");
    return true;
  }

  /** Cascade `dependent: :destroy` children first (so each child's
   *  own destroy logic runs), then remove this row. Fires
   *  afterDestroy + afterCommit on the destroyed record. */
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
    this._fireCallbacks("_afterDestroyCallbacks");
    this._fireCallbacks("_afterCommitCallbacks");
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

  // Callback registration. Each subclass gets its own array via the
  // `_ensureOwnCallbacks` check — without that, subclasses would share
  // the base class's array and callbacks registered for `Article` would
  // fire on `Comment` too. Pattern mirrors railcar's TS runtime.
  static _afterSaveCallbacks: Array<(record: any) => any> = [];
  static _afterDestroyCallbacks: Array<(record: any) => any> = [];
  static _afterCreateCallbacks: Array<(record: any) => any> = [];
  static _afterUpdateCallbacks: Array<(record: any) => any> = [];
  static _afterCommitCallbacks: Array<(record: any) => any> = [];

  private static _ensureOwnCallbacks(listName: string): void {
    if (!Object.prototype.hasOwnProperty.call(this, listName)) {
      (this as any)[listName] = [];
    }
  }

  static afterSave(fn: (record: any) => any): void {
    this._ensureOwnCallbacks("_afterSaveCallbacks");
    (this as any)._afterSaveCallbacks.push(fn);
  }
  static afterDestroy(fn: (record: any) => any): void {
    this._ensureOwnCallbacks("_afterDestroyCallbacks");
    (this as any)._afterDestroyCallbacks.push(fn);
  }
  static afterCreate(fn: (record: any) => any): void {
    this._ensureOwnCallbacks("_afterCreateCallbacks");
    (this as any)._afterCreateCallbacks.push(fn);
  }
  static afterUpdate(fn: (record: any) => any): void {
    this._ensureOwnCallbacks("_afterUpdateCallbacks");
    (this as any)._afterUpdateCallbacks.push(fn);
  }
  static afterCommit(fn: (record: any) => any): void {
    this._ensureOwnCallbacks("_afterCommitCallbacks");
    (this as any)._afterCommitCallbacks.push(fn);
  }

  /** Fire a callback list on this record. Walks the prototype chain
   *  so a subclass inherits its ancestors' registrations. */
  private _fireCallbacks(listName: string): void {
    let cls: any = this.constructor;
    const seen = new Set<Array<(record: any) => any>>();
    while (cls && cls !== Object) {
      if (Object.prototype.hasOwnProperty.call(cls, listName)) {
        const list = cls[listName] as Array<(record: any) => any>;
        if (!seen.has(list)) {
          seen.add(list);
          for (const cb of list) cb(this);
        }
      }
      cls = Object.getPrototypeOf(cls);
    }
  }

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

  // Turbo-stream broadcasts on the record. Each call composes a
  // `<turbo-stream action="..." target="...">` fragment and hands it
  // to the module-level `broadcaster`. When no broadcaster is
  // installed (test mode, no HTTP server), the call is a silent
  // no-op. Production server installs one via `setBroadcaster`.
  //
  // The target defaults to the record's DOM id (`article_42`);
  // overridable for collection-scoped appends (e.g., prepend a new
  // `article` fragment onto the `articles` list container — in that
  // case the stream name IS the target).
  broadcastPrependTo(stream: string, target?: string): void {
    this._broadcast("prepend", stream, target ?? stream);
  }
  broadcastAppendTo(stream: string, target?: string): void {
    this._broadcast("append", stream, target ?? stream);
  }
  broadcastReplaceTo(stream: string, target?: string): void {
    this._broadcast("replace", stream, target ?? this._domId());
  }
  broadcastRemoveTo(stream: string, target?: string): void {
    this._broadcast("remove", stream, target ?? this._domId());
  }
  broadcastUpdateTo(stream: string, target?: string): void {
    this._broadcast("update", stream, target ?? this._domId());
  }

  private _domId(): string {
    const cls = this.constructor as typeof ApplicationRecord;
    const base = cls.table_name.replace(/s$/, "");
    return `${base}_${this.id}`;
  }

  private _broadcast(action: string, stream: string, target: string): void {
    if (!broadcaster) return;
    let html = "";
    if (action !== "remove") {
      const cls = this.constructor as typeof ApplicationRecord;
      if (cls.renderPartial) {
        try {
          html = cls.renderPartial(this);
        } catch (_) {
          html = "";
        }
      }
    }
    const body = action === "remove"
      ? `<turbo-stream action="remove" target="${target}"></turbo-stream>`
      : `<turbo-stream action="${action}" target="${target}"><template>${html}</template></turbo-stream>`;
    broadcaster(stream, body);
  }

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

/** Metadata describing an association, produced by the emitter
 *  from `has_many` / `has_one` declarations. `foreignKey` is the
 *  column in the target model's table that stores the owner's id;
 *  `name` is the Ruby-side association name (used in diagnostics).
 */
export interface AssocConfig {
  name: string;
  type: "has_many" | "has_one" | "belongs_to";
  foreignKey: string;
}

/** Runtime proxy for `has_many` associations. Lazy — each property
 *  access issues a `SELECT ... WHERE fk = ?` against the target
 *  table. Doesn't cache; callers who iterate multiple times pay
 *  multiple queries. That's fine for simple scaffolds; production-
 *  scale callers would materialize `.all` once into a local.
 */
export class CollectionProxy<T extends ApplicationRecord = ApplicationRecord> {
  private owner: ApplicationRecord;
  private config: AssocConfig;
  private target: typeof ApplicationRecord;

  constructor(owner: ApplicationRecord, config: AssocConfig, target: typeof ApplicationRecord) {
    this.owner = owner;
    this.config = config;
    this.target = target;
  }

  /** All rows where `foreignKey = owner.id`. Materializes on each
   *  call — use `const list = proxy.all;` and iterate the local. */
  get all(): T[] {
    const rows = conn()
      .prepare(
        `SELECT * FROM ${this.target.table_name} WHERE ${this.config.foreignKey} = ?`,
      )
      .all(this.owner.id) as Record<string, any>[];
    return rows.map((row) => Object.assign(new (this.target as any)(), row)) as T[];
  }

  get size(): number {
    const row = conn()
      .prepare(
        `SELECT COUNT(*) AS c FROM ${this.target.table_name} WHERE ${this.config.foreignKey} = ?`,
      )
      .get(this.owner.id) as { c: number };
    return row.c;
  }

  get length(): number {
    return this.size;
  }

  get any(): boolean {
    return this.size > 0;
  }

  /** `collection.build(attrs)` — construct a new child with the
   *  FK pre-set, unsaved. Used by scaffolded create paths and by
   *  emitted association-fill patterns. */
  build(attrs: Record<string, any> = {}): T {
    const child = new (this.target as any)() as any;
    Object.assign(child, attrs);
    child[this.config.foreignKey] = this.owner.id;
    return child;
  }

  /** `collection.create(attrs)` — build + save. Caller inspects
   *  `record.errors.any` for validation-failure detection. */
  create(attrs: Record<string, any> = {}): T {
    const child = this.build(attrs);
    (child as any).save;
    return child;
  }

  each(fn: (item: T) => void): void {
    for (const item of this.all) fn(item);
  }

  map<U>(fn: (item: T) => U): U[] {
    return this.all.map(fn);
  }

  forEach(fn: (item: T) => void): void {
    this.each(fn);
  }

  [Symbol.iterator](): Iterator<T> {
    return this.all[Symbol.iterator]();
  }
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
    const handler = Router.resolveHandler(controller, action);
    if (!handler) return;
    Router.routes.push({ method: "GET", path, handler });
  }

  /** Look up an action on a controller. The emitter may export the
   *  `new` action either as `new` (scaffold convention) or `$new`
   *  (reserved-word-escape convention). Accept either. */
  private static resolveHandler(controller: any, action: string): Route["handler"] | undefined {
    return controller[action] ?? (action === "new" ? controller["$new"] : undefined);
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
      const handler = Router.resolveHandler(controller, action);
      if (!handler) continue;
      Router.routes.push({ method, path, handler });
    }
  }

  static get(path: string, controller: any, action: string): void {
    const handler = Router.resolveHandler(controller, action);
    if (handler) Router.routes.push({ method: "GET", path, handler });
  }
  static post(path: string, controller: any, action: string): void {
    const handler = Router.resolveHandler(controller, action);
    if (handler) Router.routes.push({ method: "POST", path, handler });
  }
  static put(path: string, controller: any, action: string): void {
    const handler = Router.resolveHandler(controller, action);
    if (handler) Router.routes.push({ method: "PUT", path, handler });
  }
  static patch(path: string, controller: any, action: string): void {
    const handler = Router.resolveHandler(controller, action);
    if (handler) Router.routes.push({ method: "PATCH", path, handler });
  }
  static delete(path: string, controller: any, action: string): void {
    const handler = Router.resolveHandler(controller, action);
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
