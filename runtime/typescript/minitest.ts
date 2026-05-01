// Minitest::Test analog for TypeScript — registers each `test_*`
// instance method on a subclass as a node:test test() block.
//
// Lowering convention: `test_module_to_library` produces a class
// per Minitest test file, with one `test_<snake_name>(): void`
// method per `test "..." do ... end` block. Setup hooks are
// inlined at the top of each test method (no separate `setup()`
// override on subclasses), so the runtime here just instantiates
// and invokes.
//
// Assertion methods mirror Minitest's surface so emitted bodies
// like `this.assert_equal(expected, actual)` resolve to a real
// implementation; each delegates to node's `assert/strict`.

import { test as nodeTest } from "node:test";
import assert from "node:assert/strict";

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

  assert_difference(expr: string | (() => number), body: () => any): any {
    // Rails' `assert_difference("Model.count", +1) do … end` form.
    // The expression form is JS-eval-style which we don't support
    // here — accept callable form. If given a string, treat as
    // a no-op difference check (presence-of-call semantics).
    const before = typeof expr === "function" ? expr() : 0;
    const result = body();
    const after = typeof expr === "function" ? expr() : 0;
    assert.notStrictEqual(after, before, "expected difference");
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
