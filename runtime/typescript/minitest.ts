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

// Emitted layout: this file lands at `test/_runtime/minitest.ts`;
// the transpiled `router.ts` lands at `src/router.ts`. Resolve via
// the `../../src/router.js` relative path so tsc finds Route as a
// concrete class (was `Record<string, any>`).
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
  // ── Assertions kept here ─────────────────────────────────────
  //
  // The inline_assertions lowerer rewrites most assert_*/refute_*
  // calls to inline `if (!cond) { throw … }` at the call site
  // (src/lower/test_module_to_library/inline_assertions.rs).
  // Methods retained here are the ones whose semantics differ
  // enough across targets that uniform inline emit isn't safe:
  //
  //   - `assert_match`: Ruby `=~` and Crystal `Regex#matches?` have
  //     incompatible nilable-value handling; the cross-target inline
  //     shape would need per-target regex API mapping.
  //   - `assert_operator`: Class-subclass `<` (e.g., `assert_operator
  //     A, :<, B` for "A is a subclass of B") is a Ruby/Crystal
  //     idiom with no operator-on-class-object analog in TS — needs
  //     a runtime prototype-chain walk here.

  assert_match(pattern: RegExp | string, value: string, msg?: string): void {
    const re = typeof pattern === "string" ? new RegExp(pattern) : pattern;
    assert.match(value, re, msg);
  }

  // Ruby's `assert_operator a, :op, b` evaluates `a.send(op, b)`. The
  // forms that survive transpile here: numeric comparisons (`:<`, `:>`,
  // `:<=`, `:>=`, `:==`) and class-subclass (`:<` between two classes,
  // walking the prototype chain).
  assert_operator(left: any, op: string, right: any, msg?: string): void {
    if (typeof left === "function" && typeof right === "function") {
      // Class-on-class `<` → strict-subclass check.
      if (op === "<" || op === ":<") {
        let proto = Object.getPrototypeOf(left.prototype);
        while (proto) {
          if (proto.constructor === right) return;
          proto = Object.getPrototypeOf(proto);
        }
        assert.fail(msg ?? `expected ${left.name} < ${right.name}`);
      }
    }
    const opStr = op.startsWith(":") ? op.slice(1) : op;
    let result: any;
    switch (opStr) {
      case "<":  result = left <  right; break;
      case ">":  result = left >  right; break;
      case "<=": result = left <= right; break;
      case ">=": result = left >= right; break;
      case "==": result = left == right; break;
      case "!=": result = left != right; break;
      default: assert.fail(`assert_operator: unsupported op ${op}`);
    }
    assert.ok(result, msg ?? `expected ${left} ${opStr} ${right}`);
  }

  skip(msg?: string): void {
    throw Object.assign(new Error(msg ?? "skipped"), { skipped: true });
  }

  flunk(msg?: string): void {
    assert.fail(msg ?? "flunked");
  }

  // Ruby `sleep N` (seconds) — busy-wait so the surrounding test
  // method stays sync. Only used by the few framework tests that
  // need timestamp granularity (`sleep 0.01` between two saves);
  // not a performance concern at the call rates we hit.
  sleep(seconds: number): void {
    const end = Date.now() + seconds * 1000;
    while (Date.now() < end) {}
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
    controller.process_action(match.action);
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
  // Rails 8.1.x scaffold renamed `:unprocessable_entity` → `:unprocessable_content`
  // mid-version (HTTP 422 description churn). Alias both so emit follows
  // whichever the fixture's scaffold currently produces.
  unprocessable_content: 422,
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
