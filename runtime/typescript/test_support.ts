// Roundhouse TypeScript test-support runtime.
//
// Hand-written, shipped alongside generated code (copied in by the
// TS emitter as `src/test_support.ts`). Controller tests call into
// the `TestClient` class for HTTP dispatch (pure in-process — no
// real server) and the `TestResponse` wrapper for Rails-compatible
// assertions (`assertOk`, `assertRedirectedTo`, `assertSelect`).
//
// Mirrors `runtime/rust/test_support.rs` in intent, shape, and
// assertion semantics — substring-match on the response body,
// loose-but-reliable for the scaffold blog's HTML. A later phase
// can swap in a real HTML parser (cheerio, linkedom, jsdom) by
// touching only this file; emitted test bodies are insulated via
// the TestResponse method contracts.

import { Router } from "./router.js";
import { Parameters } from "./parameters.js";
import { type ActionResponse } from "./juntos.js";

/** Test fixtures need to install the dispatch table + controller
 *  registry before issuing requests. Mirrors `startServer`'s shape;
 *  test setup calls `installRoutes(Routes.table(), Routes.root(),
 *  controllers)` once before any TestClient request. */
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

/** Pure-TS test client — dispatches through `Router.match`,
 *  instantiates the matched controller, calls `process_action`,
 *  wraps the response. No real HTTP, no socket setup. */
export class TestClient {
  async get(path: string): Promise<TestResponse> {
    return this.dispatch("GET", path, {});
  }

  async post(path: string, body: Record<string, string> = {}): Promise<TestResponse> {
    return this.dispatch("POST", path, body);
  }

  async patch(path: string, body: Record<string, string> = {}): Promise<TestResponse> {
    return this.dispatch("PATCH", path, body);
  }

  async delete(path: string): Promise<TestResponse> {
    return this.dispatch("DELETE", path, {});
  }

  private async dispatch(
    method: string,
    path: string,
    body: Record<string, string>,
  ): Promise<TestResponse> {
    const match = Router.match(method, path, testDispatchTable);
    if (!match) {
      throw new Error(`no route for ${method} ${path}`);
    }
    const ctrlClass = testControllerRegistry[match.controller];
    if (!ctrlClass) {
      throw new Error(`no controller registered for ${match.controller}`);
    }
    const merged: Record<string, any> = { ...match.path_params, ...body };
    const controller = new ctrlClass();
    controller.params = new Parameters(merged);
    controller.session = {};
    controller.flash = {};
    controller.request_method = method;
    controller.request_path = path;
    await controller.process_action(match.action);
    const response: ActionResponse = {
      body: controller.body,
      status: controller.status,
      location: controller.location,
    };
    return new TestResponse(response);
  }
}

/** Wrapper around `ActionResponse` exposing assertion helpers.
 *  Method names mirror Rails' Minitest HTTP assertions; bodies
 *  substring-match for `assertSelect`-style queries. */
export class TestResponse {
  readonly body: string;
  readonly status: number;
  readonly location: string;

  constructor(raw: ActionResponse) {
    this.body = raw.body ?? "";
    this.status = raw.status ?? 200;
    this.location = raw.location ?? "";
  }

  /** `assert_response :success` — status 200 OK. */
  assertOk(): void {
    if (this.status !== 200) {
      throw new Error(`expected 200 OK, got ${this.status}`);
    }
  }

  /** `assert_response :unprocessable_entity` — status 422. */
  assertUnprocessable(): void {
    if (this.status !== 422) {
      throw new Error(`expected 422 Unprocessable Entity, got ${this.status}`);
    }
  }

  /** `assert_response <code>`. */
  assertStatus(code: number): void {
    if (this.status !== code) {
      throw new Error(`expected status ${code}, got ${this.status}`);
    }
  }

  /** `assert_redirected_to <path>` — response status is a 3xx and
   *  the Location header substring-matches the expected path.
   *  Loose to tolerate absolute-vs-relative URL differences. */
  assertRedirectedTo(path: string): void {
    if (this.status < 300 || this.status >= 400) {
      throw new Error(`expected a redirection, got ${this.status}`);
    }
    if (!this.location.includes(path)) {
      throw new Error(
        `expected Location to contain ${JSON.stringify(path)}, got ${JSON.stringify(this.location)}`,
      );
    }
  }

  /** `assert_select <selector>` — body contains a match for the
   *  selector. Substring-matches on the opening tag or
   *  `id=`/`class=` attribute fragment. Covers the scaffold
   *  blog's shapes:
   *    "h1"            → contains "<h1"
   *    "#articles"     → contains `id="articles"`
   *    ".p-4"          → contains `p-4"`
   *    "form"          → contains "<form" */
  assertSelect(selector: string): void {
    const fragment = selectorFragment(selector);
    if (!this.body.includes(fragment)) {
      throw new Error(
        `expected body to match selector ${JSON.stringify(selector)} (looked for ${JSON.stringify(fragment)})`,
      );
    }
  }

  /** `assert_select <selector>, <text>` — selector check + body
   *  also contains `text`. Phase 4d doesn't scope the text to the
   *  selector match (would need real HTML parsing). */
  assertSelectText(selector: string, text: string): void {
    this.assertSelect(selector);
    if (!this.body.includes(text)) {
      throw new Error(
        `expected body to contain text ${JSON.stringify(text)} under selector ${JSON.stringify(selector)}`,
      );
    }
  }

  /** `assert_select <selector>, minimum: N` — at least `n`
   *  occurrences of the selector fragment. */
  assertSelectMin(selector: string, n: number): void {
    const fragment = selectorFragment(selector);
    let count = 0;
    let from = 0;
    while (true) {
      const i = this.body.indexOf(fragment, from);
      if (i < 0) break;
      count++;
      from = i + fragment.length;
    }
    if (count < n) {
      throw new Error(
        `expected at least ${n} matches for selector ${JSON.stringify(selector)}, got ${count}`,
      );
    }
  }
}

/** Turn a loose selector into a substring fragment that probably
 *  appears in matching HTML. Same rules as Rust's twin — `#id` →
 *  `id="id"`, `.class` → `class"`, bare tag → `<tag`. Compound
 *  selectors (`"#comments .p-4"`) split and pick the first chunk. */
function selectorFragment(selector: string): string {
  const first = selector.split(/\s+/)[0] ?? "";
  if (first.startsWith("#")) return `id="${first.slice(1)}"`;
  if (first.startsWith(".")) return `${first.slice(1)}"`;
  return `<${first}`;
}
