// Node-side validation: load the committed wasm + the generated fixture.json,
// run the SAME shared driver the browser uses, and assert the output is a
// complete full-stack emission. Confirms the lean fixture is sufficient before
// we ship it to the browser page.
//
// Asserted as a floor + key-file presence rather than an exact count: the TS
// emit pipeline grows (app + src/ runtime + tests + per-file sourcemaps +
// config — currently ~79 files for real-blog), so an exact count is brittle.
//
// Run from this directory: node validate-fixture.mjs

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { loadCompiler } from "./transpile.mjs";

const WASM = resolve("./roundhouse_wasm.wasm");
const FIXTURE = resolve("./fixture.json");
const MIN_FILES = 50;
const KEY_FILES = [
  "app/models/article.ts",
  "app/controllers/articles_controller.ts",
  "app/views/articles/index.ts",
  "src/router.ts",
  "test/article.test.ts",
  "main.ts",
];

const wasmBytes = await readFile(WASM);
const srcMap = JSON.parse(await readFile(FIXTURE, "utf8"));
console.log(`fixture: ${Object.keys(srcMap).length} files`);

const compiler = await loadCompiler(wasmBytes);

const t0 = performance.now();
const result = compiler.transpile("typescript", srcMap);
const t1 = performance.now();

if (result.error) {
  console.error(`ERROR: ${result.error}`);
  process.exit(1);
}
console.log(`transpiled in ${(t1 - t0).toFixed(1)}ms → ${result.files.length} files`);
for (const f of result.files) {
  console.log(`  ${f.path}  (${f.content.length} bytes)`);
}

const paths = new Set(result.files.map((f) => f.path));
const missing = KEY_FILES.filter((p) => !paths.has(p));
let ok = true;
if (result.files.length < MIN_FILES) {
  console.error(`\nFAIL: expected >= ${MIN_FILES} files, got ${result.files.length}`);
  ok = false;
}
if (missing.length) {
  console.error(`\nFAIL: missing key files: ${missing.join(", ")}`);
  ok = false;
}
if (!ok) process.exit(1);
console.log(`\nOK: ${result.files.length} files incl. all ${KEY_FILES.length} key files`);
