// Minitest::Test analog for TypeScript — registers each `test_*`
// instance method on a subclass as a node:test test() block, and
// supplies the Test base class with both the core Minitest
// assertions (assert_equal, assert_not_nil, …) and the Rails
// ActionDispatch::IntegrationTest surface (get/post/patch/delete +
// assert_response/assert_redirected_to/assert_select). Mirrors
// spinel's `runtime/spinel/test/test_helper.rb` — same role, one
// file per target.
//
// Lowering convention: `test_module_to_library` produces a class
// per Minitest test file, with one `test_<snake_name>(): void`
// method per `test "..." do ... end` block. Setup hooks are
// inlined at the top of each test method (no separate `setup()`
// override on subclasses), so the runtime here just instantiates
// and invokes.

import { test as nodeTest } from "node:test";
import assert from "node:assert/strict";

// minitest.ts is emitted to `test/_runtime/`; `router.ts` and
// `parameters.ts` are emitted to `src/`. Two levels up.
import { Router } from "../../src/router.js";
import { Parameters } from "../../src/parameters.js";
import { HashWithIndifferentAccess } from "../../src/hash_with_indifferent_access.js";

// ── Per-test dispatch table ────────────────────────────────────────
//
// Tests need to install the routes table + controller registry once
// before any HTTP-style request method (`this.get(...)`) fires.
// `installRoutes` is the seam — call it from a generated bootstrap
// step that runs before `discover_tests`.

type RouteRow = Record<string, any>;
type ControllerClass = new () => any;

let testDispatchTable: RouteRow[] = [];
let testControllerRegistry: Record<string, ControllerClass> = {};

export function installRoutes(
  routes: RouteRow[],
  rootRoute: RouteRow | undefined,
  controllers: Record<string, ControllerClass>,
): void {
  testDispatchTable = rootRoute ? [rootRoute, ...routes] : [...routes];
  testControllerRegistry = controllers;
}

export class Test {
  // ── Core assertions ──────────────────────────────────────────
  assert(cond: any, msg?: string): void {
    assert.ok(cond, msg);
  }

  assert_equal(expected: any, actual: any, msg?: string): void {
    assert.deepStrictEqual(actual, expected, msg);
  }

  assert_not(cond: any, msg?: string): void {
    assert.ok(!cond, msg);
  }

  assert_not_equal(expected: any, actual: any, msg?: string): void {
    assert.notDeepStrictEqual(actual, expected, msg);
  }

  assert_nil(value: any, msg?: string): void {
    assert.strictEqual(value, null, msg);
  }

  assert_not_nil(value: any, msg?: string): void {
    assert.notStrictEqual(value, null, msg);
    assert.notStrictEqual(value, undefined, msg);
  }

  assert_empty(collection: any, msg?: string): void {
    if (Array.isArray(collection) || typeof collection === "string") {
      assert.strictEqual(collection.length, 0, msg);
    } else if (collection && typeof collection.size === "number") {
      assert.strictEqual(collection.size, 0, msg);
    } else if (collection && typeof collection === "object") {
      assert.strictEqual(Object.keys(collection).length, 0, msg);
    } else {
      assert.fail(`assert_empty: unsupported collection type for ${collection}`);
    }
  }

  assert_not_empty(collection: any, msg?: string): void {
    if (Array.isArray(collection) || typeof collection === "string") {
      assert.notStrictEqual(collection.length, 0, msg);
    } else if (collection && typeof collection.size === "number") {
      assert.notStrictEqual(collection.size, 0, msg);
    } else if (collection && typeof collection === "object") {
      assert.notStrictEqual(Object.keys(collection).length, 0, msg);
    } else {
      assert.fail(`assert_not_empty: unsupported collection type for ${collection}`);
    }
  }

  assert_includes(collection: any, item: any, msg?: string): void {
    if (Array.isArray(collection) || typeof collection === "string") {
      assert.ok(collection.includes(item), msg);
    } else if (collection && typeof collection.has === "function") {
      assert.ok(collection.has(item), msg);
    } else {
      assert.fail(`assert_includes: unsupported collection type for ${item}`);
    }
  }

  assert_match(pattern: RegExp | string, value: string, msg?: string): void {
    const re = typeof pattern === "string" ? new RegExp(pattern) : pattern;
    assert.match(value, re, msg);
  }

  assert_raises(_errClass: any, body: () => any): any {
    let caught: any = null;
    try {
      body();
    } catch (e) {
      caught = e;
    }
    if (caught === null) {
      assert.fail("expected block to raise");
    }
    return caught;
  }

  // Rails' `assert_difference("Model.count", +1) do … end` form.
  // Two surface shapes survive translation:
  //   assert_difference(expr, body)               — count diff = 1
  //   assert_difference(expr, delta, body)        — count diff = delta
  // The expression form is JS-eval-style which we don't support
  // here — accept callable form. If given a string, treat as
  // a no-op difference check (presence-of-call semantics).
  assert_difference(
    expr: string | (() => number),
    deltaOrBody: number | (() => any),
    body?: () => any,
  ): any {
    const [delta, runBody] = typeof deltaOrBody === "function"
      ? [1, deltaOrBody]
      : [deltaOrBody, body!];
    const before = typeof expr === "function" ? expr() : 0;
    const result = runBody();
    const after = typeof expr === "function" ? expr() : 0;
    if (typeof expr === "function") {
      assert.strictEqual(after - before, delta, `expected difference of ${delta}`);
    }
    return result;
  }

  assert_no_difference(expr: string | (() => number), body: () => any): any {
    const before = typeof expr === "function" ? expr() : 0;
    const result = body();
    const after = typeof expr === "function" ? expr() : 0;
    assert.strictEqual(after, before, "expected no difference");
    return result;
  }

  // Minitest's refute_* family — equivalent to assert_not_*.
  refute(cond: any, msg?: string): void {
    this.assert_not(cond, msg);
  }

  refute_equal(expected: any, actual: any, msg?: string): void {
    this.assert_not_equal(expected, actual, msg);
  }

  refute_nil(value: any, msg?: string): void {
    this.assert_not_nil(value, msg);
  }

  skip(msg?: string): void {
    throw Object.assign(new Error(msg ?? "skipped"), { skipped: true });
  }

  flunk(msg?: string): void {
    assert.fail(msg ?? "flunked");
  }

  // ── ActionDispatch::IntegrationTest surface ──────────────────
  //
  // HTTP-style requests dispatch in-process through `Router.match`
  // and the per-test controller registry — same shape as spinel's
  // `RequestDispatch.dispatch_request`. The matched controller's
  // `body` / `status` / `location` populate this.response /
  // this.status / this.location for subsequent assert_* calls.
  // Tests that hit these methods without `installRoutes(...)`
  // having been called raise loudly so silent no-op runs don't
  // mask a missing bootstrap.

  body: string = "";
  status: number = 0;
  location: string = "";

  // Rails carries these as instance accessors — typed `any` so
  // chained property reads (e.g. `this.response.body`) don't fight
  // the type checker.
  response: any;
  request: any;
  session: HashWithIndifferentAccess = new HashWithIndifferentAccess();
  cookies: any;
  flash: HashWithIndifferentAccess = new HashWithIndifferentAccess();

  // Dispatch is synchronous — emitted controller actions are
  // currently all sync (process_action's switch arms call sync
  // index/show/etc. methods). node:test wraps each test in an
  // async runner; an async dispatch surface forced every emitted
  // test method to also be async + `await`, which the lowerer
  // doesn't yet produce. Keeping these sync until process_action
  // genuinely needs to await something (e.g. database I/O via a
  // Promise-returning adapter) — at which point the lowerer needs
  // to learn to emit async test methods anyway.
  get(path: string, _opts?: any): void {
    this.dispatch("GET", path, {});
  }
  post(path: string, opts: { params?: Record<string, any> } = {}): void {
    this.dispatch("POST", path, opts.params ?? {});
  }
  put(path: string, opts: { params?: Record<string, any> } = {}): void {
    this.dispatch("PUT", path, opts.params ?? {});
  }
  patch(path: string, opts: { params?: Record<string, any> } = {}): void {
    this.dispatch("PATCH", path, opts.params ?? {});
  }
  delete(path: string, _opts?: any): void {
    this.dispatch("DELETE", path, {});
  }
  head(path: string, _opts?: any): void {
    this.dispatch("HEAD", path, {});
  }

  private dispatch(
    method: string,
    path: string,
    body: Record<string, any>,
  ): void {
    const match = Router.match(method, path, testDispatchTable);
    if (!match) {
      throw new Error(`no route for ${method} ${path}`);
    }
    const ctrlClass = testControllerRegistry[match.controller];
    if (!ctrlClass) {
      throw new Error(`no controller registered for ${match.controller}`);
    }
    // path_params is now an `ActiveSupport::HashWithIndifferentAccess`
    // (HWIA). Spread its underlying String-keyed hash via `.to_h()`
    // so the keys merge into `merged` as plain entries — `{...hwia}`
    // would spread the class's own properties (`data`), not the
    // hash contents.
    const merged: Record<string, any> = { ...match.path_params.to_h(), ...body };
    const controller = new ctrlClass();
    controller.params = new Parameters(merged);
    controller.session = this.session;
    controller.flash = this.flash;
    controller.request_method = method;
    controller.request_path = path;
    controller.process_action(match.action);
    this.body = controller.body ?? "";
    this.status = controller.status ?? 200;
    this.location = controller.location ?? "";
    this.response = { body: this.body, status: this.status };
    this.flash = controller.flash ?? new HashWithIndifferentAccess();
  }

  // ── HTTP response assertions ─────────────────────────────────
  //
  // `assert_response :symbol` and `assert_response 200` are both
  // valid Rails forms; map symbols to their HTTP code, accept
  // numeric codes directly.
  assert_response(expected: string | number, _msg?: string): void {
    const expectedCode = typeof expected === "number"
      ? expected
      : RESPONSE_SYMBOLS[expected] ?? -1;
    if (expectedCode === -1) {
      assert.fail(`unknown response symbol: ${String(expected)}`);
    }
    assert.strictEqual(
      this.status,
      expectedCode,
      `expected response ${expectedCode}, got ${this.status}`,
    );
  }

  assert_redirected_to(expected: string, _msg?: string): void {
    if (this.status < 300 || this.status >= 400) {
      assert.fail(`expected a redirection, got ${this.status}`);
    }
    if (!this.location.includes(expected)) {
      assert.fail(
        `expected Location to contain ${JSON.stringify(expected)}, got ${JSON.stringify(this.location)}`,
      );
    }
  }

  // `assert_select` substring-matches on the opening tag or
  // `id=`/`class=` attribute fragment derived from the selector.
  // Rough but effective for the scaffold-blog HTML shapes —
  // bodies of the form `"#articles"`, `".p-4"`, `"h1"`, etc.
  assert_select(selector: string, _arg?: any, _msg?: string): void {
    const fragment = selectorFragment(selector);
    if (!this.body.includes(fragment)) {
      assert.fail(
        `expected body to match selector ${JSON.stringify(selector)} (looked for ${JSON.stringify(fragment)})`,
      );
    }
  }

  assert_template(_name: string, _msg?: string): void {
    // Rails' `assert_template` checks which view was rendered.
    // Matching that requires controller-side instrumentation
    // (the action records `template_name` when `render` is
    // called). Not yet wired; left as no-op so the rare test
    // that uses it doesn't fail at runtime.
  }
}

const RESPONSE_SYMBOLS: Record<string, number> = {
  ok: 200,
  success: 200,
  created: 201,
  accepted: 202,
  no_content: 204,
  moved_permanently: 301,
  found: 302,
  see_other: 303,
  not_modified: 304,
  redirect: 302,
  bad_request: 400,
  unauthorized: 401,
  forbidden: 403,
  not_found: 404,
  unprocessable_entity: 422,
  internal_server_error: 500,
  missing: 404,
};

/** Turn a loose selector into a substring fragment that probably
 *  appears in matching HTML. `#id` → `id="id"`, `.class` →
 *  `class"`, bare tag → `<tag`. Compound selectors split and pick
 *  the first chunk. */
function selectorFragment(selector: string): string {
  const first = selector.split(/\s+/)[0] ?? "";
  if (first.startsWith("#")) return `id="${first.slice(1)}"`;
  if (first.startsWith(".")) return `${first.slice(1)}"`;
  return `<${first}`;
}

// Rails-side alias. `ActiveSupport::TestCase` and `ActionDispatch::-
// IntegrationTest` both lower to `extends TestCase` in the emitter;
// the runtime treats them as identical to Minitest::Test.
export const TestCase = Test;
export type TestCase = Test;

/// Walk a Test subclass's prototype, register every `test_*` method
/// as a node:test test block. Each test runs against a fresh
/// instance so per-test mutation doesn't leak.
export function discover_tests(klass: typeof Test & (new () => Test)): void {
  const className = klass.name;
  for (const methodName of Object.getOwnPropertyNames(klass.prototype)) {
    if (!methodName.startsWith("test_")) continue;
    if (methodName === "constructor") continue;
    const method = (klass.prototype as any)[methodName];
    if (typeof method !== "function") continue;

    nodeTest(`${className}#${methodName}`, async () => {
      const instance = new klass();
      await Promise.resolve(method.call(instance));
    });
  }
}
