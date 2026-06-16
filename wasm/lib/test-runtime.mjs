// In-browser test-runner harness for /studio/ (rung D.2, Phase 7).
//
// roundhouse already transpiles the Rails Minitest suites to TS under the
// worker profile (test/<x>.test.ts + test/_runtime/minitest.ts + setup.ts +
// test/fixtures/*.ts). Those run in CI under `node:test` over better-sqlite3.
// The browser has neither. This module supplies the browser equivalents WITHOUT
// touching the emitter — the emitted suite stays byte-identical to CI (the
// plan's option (a)). Everything here is injected at BUNDLE time as virtual
// modules / srcMap overrides (see bundle.mjs `bundleTests`):
//
//   node:test          → a tiny registry+runner (registers each test, runs
//                        them sequentially, collects pass/fail/skip).
//   node:assert/strict → the assert surface minitest-async.ts calls.
//   src/db.js          → in-memory `Db` over sqlite_wasm_engine (replaces the
//                        worker-MessagePort proxy db-worker-proxy.ts).
//   src/juntos.js      → setupTestDb (in-memory engine init + schema) + a
//                        no-op broadcast (the only two VALUES the test graph
//                        imports from juntos; the rest are erased types).
//
// DB isolation (risk #4): the engine's `opfs:false` path opens a fresh
// in-memory `sqlite3.oo1.DB` — never the live app's opfs pool. A run can't
// touch the studio app's data. Both overrides import the SAME
// `src/sqlite_wasm_engine.ts` module, so esbuild dedupes it to one singleton:
// setupTestDb initializes it, `Db.*` queries it.

// ── node:test shim ────────────────────────────────────────────────────────
// minitest-async.ts does `import { test as nodeTest } from "node:test"` and
// registers each `test_*` method as `nodeTest(name, fn)`. We collect them and
// run on demand. `__runTests()` is called by the synthesized entry.
export const NODE_TEST_SRC = `
const __tests = [];
export function test(name, fn) { __tests.push({ name, fn }); }
export default test;

export async function __runTests() {
  const results = [];
  for (const t of __tests) {
    const start = performance.now();
    try {
      await t.fn();
      results.push({ name: t.name, status: "pass", ms: performance.now() - start });
    } catch (e) {
      const skipped = e && typeof e === "object" && e.skipped;
      results.push({
        name: t.name,
        status: skipped ? "skip" : "fail",
        ms: performance.now() - start,
        error: skipped ? undefined : (e && e.message ? e.message : String(e)),
      });
    }
  }
  return results;
}
`;

// ── node:assert/strict shim ───────────────────────────────────────────────
// Only the methods minitest-async.ts reaches at runtime (most assert_* are
// rewritten inline by the inline_assertions lowerer to `if (...) throw`).
export const NODE_ASSERT_SRC = `
class AssertionError extends Error {
  constructor(msg) { super(msg); this.name = "AssertionError"; }
}
function deepEqual(a, b) {
  if (a === b) return true;
  if (a == null || b == null || typeof a !== "object" || typeof b !== "object") return false;
  const ka = Object.keys(a), kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => deepEqual(a[k], b[k]));
}
function assert(value, msg) { if (!value) throw new AssertionError(msg || "assertion failed"); }
assert.ok = assert;
assert.fail = (msg) => { throw new AssertionError(typeof msg === "string" ? msg : "failed"); };
assert.equal = (a, b, msg) => { if (a != b) throw new AssertionError(msg || (a + " != " + b)); };
assert.notEqual = (a, b, msg) => { if (a == b) throw new AssertionError(msg || (a + " == " + b)); };
assert.strictEqual = (a, b, msg) => { if (a !== b) throw new AssertionError(msg || (JSON.stringify(a) + " !== " + JSON.stringify(b))); };
assert.notStrictEqual = (a, b, msg) => { if (a === b) throw new AssertionError(msg || (JSON.stringify(a) + " === " + JSON.stringify(b))); };
assert.deepEqual = (a, b, msg) => { if (!deepEqual(a, b)) throw new AssertionError(msg || "not deep-equal"); };
assert.deepStrictEqual = assert.deepEqual;
assert.match = (value, re, msg) => { const r = typeof re === "string" ? new RegExp(re) : re; if (!r.test(value)) throw new AssertionError(msg || (JSON.stringify(value) + " does not match " + r)); };
assert.AssertionError = AssertionError;
export default assert;
`;

// ── src/db.ts override: in-memory Db over the sqlite-wasm engine ───────────
// Same `Db` namespace surface as db-worker-proxy.ts (the emitted worker
// proxy), but each statement runs DIRECTLY against the in-memory engine
// (synchronous) instead of round-tripping a dedicated db_worker over a
// MessagePort. The emitted models `await Db.prepare(...)` / `await Db.exec(...)`;
// awaiting a sync value is a no-op, so the call sites are unchanged. Statement
// caching / step / column reads are identical to the proxy.
export const IN_MEMORY_DB_SRC = `
import { exec as __engineExec } from "./sqlite_wasm_engine.js";

const _statements = new Map();
let _nextId = 0;
let _lastReply = { rows: [], changes: 0, lastInsertRowId: null };

function run(sql) { return __engineExec(sql, []); }

function configure(_path) {}
function install(_handle) {}
function close() { _statements.clear(); _lastReply = { rows: [], changes: 0, lastInsertRowId: null }; }

function exec(sql) { _lastReply = run(sql); }

// Eager-run SELECT so step?/column_* stay sync; cache rows in column order
// (sqlite-wasm object rowMode preserves SELECT column order).
function prepare(sql) {
  const reply = run(sql);
  _lastReply = reply;
  const rows = reply.rows.map((r) => Object.values(r));
  _nextId += 1;
  _statements.set(_nextId, { rows, cursor: -1 });
  return _nextId;
}

function is_step(stmtId) {
  const entry = _statements.get(stmtId);
  if (entry === undefined) throw new Error("Db: unknown stmt id " + stmtId);
  entry.cursor += 1;
  return entry.cursor < entry.rows.length;
}

function column_int(stmtId, i) {
  const entry = _statements.get(stmtId);
  if (entry === undefined || entry.cursor < 0 || entry.cursor >= entry.rows.length) throw new Error("Db: column_int with no current row");
  const v = entry.rows[entry.cursor][i];
  if (v === null || v === undefined) return 0;
  if (typeof v === "bigint") return Number(v);
  return typeof v === "number" ? Math.trunc(v) : Number(v) | 0;
}

function column_text(stmtId, i) {
  const entry = _statements.get(stmtId);
  if (entry === undefined || entry.cursor < 0 || entry.cursor >= entry.rows.length) throw new Error("Db: column_text with no current row");
  const v = entry.rows[entry.cursor][i];
  if (v === null || v === undefined) return "";
  return String(v);
}

function column_bool(stmtId, i) { return column_int(stmtId, i) !== 0; }
function finalize(stmtId) { _statements.delete(stmtId); }
function last_insert_rowid() {
  const v = _lastReply.lastInsertRowId;
  if (v === undefined || v === null) return 0;
  return typeof v === "bigint" ? Number(v) : v;
}
function changes() { return _lastReply.changes || 0; }

function escape_string(s) { return "'" + String(s == null ? "" : s).replace(/'/g, "''") + "'"; }
function escape_int(n) { const p = typeof n === "number" ? n : Number(n); return Number.isFinite(p) ? String(Math.trunc(p)) : "0"; }
function escape_int_list(ids) { return ids.length === 0 ? "NULL" : ids.map(escape_int).join(", "); }
function escape_bool(b) { return b ? "1" : "0"; }

export const Db = {
  configure, install, close, exec, prepare, is_step,
  column_int, column_text, column_bool, finalize,
  last_insert_rowid, changes,
  escape_string, escape_int, escape_int_list, escape_bool,
};
`;

// ── src/juntos.ts override: setupTestDb + broadcast no-op ──────────────────
// The test graph imports only these two VALUES from juntos (setup.ts wants
// setupTestDb; broadcasts.ts wants broadcast). Everything else it imports from
// juntos is a type (erased). setupTestDb opens the in-memory engine and runs
// the schema; broadcast is a no-op (tests don't assert Turbo Stream pushes).
export const TEST_JUNTOS_SRC = `
import { initDatabase, execSQL } from "./sqlite_wasm_engine.js";

export async function setupTestDb(schema_sql) {
  await initDatabase({ opfs: false }); // in-memory — never the app's opfs pool
  execSQL(schema_sql);
}

export function broadcast(_stream, _html) {}
export function setBroadcaster(_fn) {}
export function installDb(_handle) {}
`;

// ── per-file entries ──────────────────────────────────────────────────────
// ONE entry per spec FILE, each run in its own Worker with its own fresh
// in-memory DB — mirroring `node --test`'s per-file process isolation (what
// CI uses). This matters because a spec can mutate shared fixtures (e.g.
// ArticleTest#test_destroys_comments deletes fixture article 1); bundling all
// specs into one DB would leak that across files. Each entry imports its one
// spec (which transitively runs test/_runtime/setup.ts's top-level-await DB +
// fixture load), runs it, and reports.
//
// `specPaths` are emitted spec keys like `test/article.test.ts`. Returns
// `[{ path, source, spec }]`; the entry sits under test/ so its `./x.test.js`
// specifier resolves to the emitted spec.
export function testEntries(specPaths) {
  return specPaths.map((spec) => {
    const stem = spec.replace(/^test\//, "").replace(/\.test\.ts$/, "").replace(/[^A-Za-z0-9]+/g, "_");
    const rel = "./" + spec.replace(/^test\//, "").replace(/\.ts$/, ".js");
    const path = `test/__entry__${stem}.ts`;
    const source = `import ${JSON.stringify(rel)};
import { __runTests } from "node:test";

const results = await __runTests();
const summary = {
  spec: ${JSON.stringify(spec)},
  total: results.length,
  passed: results.filter((r) => r.status === "pass").length,
  failed: results.filter((r) => r.status === "fail").length,
  skipped: results.filter((r) => r.status === "skip").length,
  results,
};
if (typeof WorkerGlobalScope !== "undefined" && typeof postMessage === "function") {
  postMessage({ type: "rh-test-results", summary });
} else {
  globalThis.__rhTestSummary = summary;
}
`;
    return { path, source, spec };
  });
}

// Bare specifiers the test bundle resolves to the shims above.
export const TEST_VIRTUALS = {
  "node:test": NODE_TEST_SRC,
  "node:assert/strict": NODE_ASSERT_SRC,
};

// srcMap overrides applied on top of the emitted worker-profile output to turn
// it into an in-browser-runnable test bundle.
export const TEST_OVERRIDES = {
  "src/db.ts": IN_MEMORY_DB_SRC,
  "src/juntos.ts": TEST_JUNTOS_SRC,
};
