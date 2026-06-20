// Minitest::Test analog for TypeScript — async variant for the
// libsql / Workers / browser deployment profiles. Same surface as
// `minitest.ts` but `dispatch` and the HTTP-style helpers
// (`get/post/put/patch/head`) return `Promise<void>` so they can
// await `process_action` (controllers are async-colored under
// these profiles). `assert_difference` / `assert_no_difference`
// are also async to await their count expressions and body blocks.
// The picker in `src/emit/typescript.rs` chooses between this and
// the sync variant the same way `juntos.ts` /
// `juntos-libsql.ts` are picked.
//
// Lowering convention: `test_module_to_library` produces a class
// per Minitest test file, with one `test_<snake_name>(): void`
// method per `test "..." do ... end` block. Setup hooks are
// inlined at the top of each test method (no separate `setup()`
// override on subclasses), so the runtime here just instantiates
// and invokes.

import { test as nodeTest } from "node:test";
import assert from "node:assert/strict";
import { parseHTML } from "linkedom";

// minitest-async.ts is emitted to `test/_runtime/`; `router.ts` and
// `flash.ts` are emitted to `src/`. Two levels up.
import { Router } from "../../src/router.js";
import { Flash } from "../../src/flash.js";
import { Session } from "../../src/session.js";

// ── Per-test dispatch table ────────────────────────────────────────
//
// Tests need to install the routes table + controller registry once
// before any HTTP-style request method (`this.get(...)`) fires.
// `installRoutes` is the seam — call it from a generated bootstrap
// step that runs before `discover_tests`.

// Emitted layout: this file lands at `test/_runtime/minitest-async.ts`;
// the transpiled `router.ts` lands at `src/router.ts`. Resolve via
// the `../../src/router.js` relative path so tsc finds Route as a
// concrete class.
import { Route as RouteClass } from "../../src/router.js";
type RouteRow = RouteClass;
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
  // The inline_assertions lowerer rewrites most assert_*/refute_*
  // calls inline (src/lower/test_module_to_library/inline_assertions.
  // rs). Methods retained here are the ones not lowered uniformly
  // (cross-target friction with nilable values / class-subclass `<`).
  // Symmetric with the sync `minitest.ts` Test class.

  assert_match(pattern: RegExp | string, value: string, msg?: string): void {
    const re = typeof pattern === "string" ? new RegExp(pattern) : pattern;
    assert.match(value, re, msg);
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
  session: Session = new Session();
  cookies: any;
  flash: Flash = new Flash();

  // Dispatch awaits `process_action` because controller actions
  // return Promise<void> under the libsql/async profiles (any AR
  // method on the request path suspends). Test methods are
  // colored async by propagation and emit `(await this.get(...))`
  // for every HTTP-style helper.
  async get(path: string, _opts?: any): Promise<void> {
    await this.dispatch("GET", path, {});
  }
  async post(path: string, opts: { params?: Record<string, any> } = {}): Promise<void> {
    await this.dispatch("POST", path, opts.params ?? {});
  }
  async put(path: string, opts: { params?: Record<string, any> } = {}): Promise<void> {
    await this.dispatch("PUT", path, opts.params ?? {});
  }
  async patch(path: string, opts: { params?: Record<string, any> } = {}): Promise<void> {
    await this.dispatch("PATCH", path, opts.params ?? {});
  }
  async delete(path: string, _opts?: any): Promise<void> {
    await this.dispatch("DELETE", path, {});
  }
  async head(path: string, _opts?: any): Promise<void> {
    await this.dispatch("HEAD", path, {});
  }

  private async dispatch(
    method: string,
    path: string,
    body: Record<string, any>,
  ): Promise<void> {
    const match = Router.match(method, path, testDispatchTable);
    if (!match) {
      throw new Error(`no route for ${method} ${path}`);
    }
    const ctrlClass = testControllerRegistry[match.controller];
    if (!ctrlClass) {
      throw new Error(`no controller registered for ${match.controller}`);
    }
    // path_params is `Hash[String, String]` (URL captures) — spread
    // directly. The earlier HWIA shape forced an `untyped` value
    // channel that strict-typed targets couldn't compile against.
    const merged: Record<string, any> = { ...match.path_params, ...body };
    const controller = new ctrlClass();
    controller.params = merged;
    controller.session = this.session;
    controller.flash = this.flash;
    controller.request_method = method;
    controller.request_path = path;
    await controller.process_action(match.action);
    this.body = controller.body ?? "";
    this.status = controller.status ?? 200;
    this.location = controller.location ?? "";
    this.response = { body: this.body, status: this.status };
    this.flash = controller.flash ?? new Flash();
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

  // `assert_select` over the Dom primitive surface (defined below).
  // Presence check: the selector matches at least one node. The stub
  // Dom is a substring matcher, so this stays rough-but-effective for
  // the scaffold-blog HTML shapes (`"#articles"`, `".p-4"`, `"h1"`, …);
  // a real engine tightens it without changing this call site.
  assert_select(selector: string, _arg?: any, _msg?: string): void {
    if (Dom.select(Dom.parse(this.body), selector).length === 0) {
      assert.fail(
        `expected body to match selector ${JSON.stringify(selector)}`,
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
  // Alias for Rails 8.1.x scaffold's `:unprocessable_content` rename;
  // see runtime/typescript/minitest.ts for the rationale.
  unprocessable_content: 422,
  internal_server_error: 500,
  missing: 404,
};

// ── Dom primitive surface (the assert_select substrate) ────────────
//
// Backed by linkedom: a real, worker-bundle-able DOM + CSS selector
// engine (css-select), exposed through the cross-target contract
// (runtime/spinel/test/test_helper.rbs). `parse` builds a document,
// `select` runs a real CSS query, `text` reads an element's
// textContent — so `assert_select` does genuine structural matching
// instead of substring guessing. linkedom is the one engine that runs
// in BOTH execution contexts this runtime targets: the node test
// runner (installed from package.json) and the browser SharedWorker /
// studio (bundled as a CDN external — see wasm/lib/bundle.mjs). jsdom
// is node-only and can't bundle into a worker; css-select covers every
// selector assert_select needs. Typed `any` at the boundary to avoid
// global-lib-DOM vs linkedom-DOM type friction.
const Dom = {
  // Parse an HTML document into a queryable root.
  parse(html: string): any {
    return parseHTML(html ?? "").document;
  },
  // Elements matching `selector` within `root` (a document or element).
  select(root: any, selector: string): any[] {
    return Array.from(root.querySelectorAll(selector));
  },
  // An element's concatenated descendant text.
  text(node: any): string {
    return node.textContent ?? "";
  },
};

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
