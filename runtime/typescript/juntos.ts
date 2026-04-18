// Roundhouse TypeScript runtime — minimal Juntos-shape stub.
//
// The TS emitter targets Juntos (https://www.ruby2js.com/docs/juntos/),
// whose runtime is a substantial npm package. For a self-contained
// generated project that type-checks under `tsc` without requiring a
// real npm install, this stub provides just enough of Juntos's surface:
// classes/types the emitted code imports. A real Juntos runtime
// swaps in via tsconfig path mapping when that's available.
//
// Everything here is intentionally permissive (`any` types, empty
// implementations) — this is for type-checking the generated code,
// not for running it. Production deployments will use the real
// Juntos package.

export class ApplicationRecord {
  attributes: Record<string, any> = {};
  errors: ErrorCollection = new ErrorCollection();

  static table_name: string = "";
  static columns: string[] = [];

  static get all(): any[] {
    return [];
  }

  static find(_id: any): any {
    return new (this as any)();
  }

  static new(_attrs?: any): any {
    return new (this as any)();
  }

  // Broadcast callback registration — the emitter renders
  // `broadcasts_to` declarations as post-class-body calls to these.
  // Real Juntos dispatches to Turbo Stream broadcasts; the stub just
  // accepts the callback and does nothing.
  static afterSave(_fn: (record: any) => any): void {}
  static afterDestroy(_fn: (record: any) => any): void {}
  static afterCreate(_fn: (record: any) => any): void {}
  static afterUpdate(_fn: (record: any) => any): void {}
  static afterCommit(_fn: (record: any) => any): void {}

  get save(): boolean {
    this.validate();
    return this.errors.none;
  }

  get destroy(): this {
    return this;
  }

  validate(): void {}

  validates_presence_of(_field: string): void {}

  validates_length_of(_field: string, _opts?: any): void {}

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
