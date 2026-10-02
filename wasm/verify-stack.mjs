// Regression for the WASI linear-stack overflow: Rust 1.99's release
// ingest frames exhausted the default 1 MiB on a 45-term || chain.
// Run after building: node wasm/verify-stack.mjs [path/to/compiler.wasm]
// Uses the browser's real C-ABI driver, not a native Rust test thread.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { loadEngine } from "./lib/engine.mjs";

const bytes = await readFile(process.argv[2] ?? new URL("./lib/roundhouse_wasm.wasm", import.meta.url));

// A reduced input also exercises ingest without the TypeScript runtime.
// Check the type/span of EVERY comparison: silently skipping the body
// or truncating the chain must not turn an overflow regression green.
const terms = Array.from({ length: 45 }, (_, i) => `name == "item${i}"`);
const chain = terms.join(" || ");
const seeds = `name = "item0"\n${chain}\n`;
const ruby = (await loadEngine(bytes)).callExport("transpile", {
  language: "ruby",
  src: { "db/seeds.rb": seeds },
});
assert.equal(ruby.error, undefined);
assert.equal(ruby.language, "ruby");
assert.ok(ruby.files.length > 0);
assert.ok(!(ruby.diagnostics ?? []).some(d => d.severity === "error"));
assert.ok(ruby.files.find(f => f.path === "db/seeds.rb")?.content.includes(chain),
  "emitted seeds must retain the complete comparison chain");
let col = 1;
for (const term of terms) {
  assert.ok(ruby.inferred_types.some(t =>
    t.path === "db/seeds.rb" && t.start_line === 2 && t.end_line === 2 &&
    t.start_col === col && t.end_col === col + term.length && t.ty === "bool"
  ), `missing typed comparison: ${term}`);
  col += term.length + " || ".length;
}

// Even an EMPTY app used to trap: TypeScript parses the embedded
// ViewHelpers.boolean_attribute? chain before runtime tree-shaking.
const ts = (await loadEngine(bytes)).callExport("transpile", {
  language: "typescript",
  src: {},
});
assert.equal(ts.error, undefined);
assert.equal(ts.language, "typescript");
assert.ok(!(ts.diagnostics ?? []).some(d => d.severity === "error"));
for (const path of ["main.ts", "src/router.ts", "src/view_helpers.ts"]) {
  assert.ok(ts.files.some(f => f.path === path && f.content.length > 0), path);
}
console.log("OK: 45 typed comparisons and empty-app TypeScript runtime emission");
