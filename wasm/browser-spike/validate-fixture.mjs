// Node-side validation: load the committed wasm + the generated fixture.json,
// run the SAME shared driver the browser uses, and assert the output matches
// the known-good baseline (15 TS files). Confirms the lean fixture is
// sufficient before we ship it to the browser page.
//
// Run from this directory: node validate-fixture.mjs

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { loadCompiler } from "./transpile.mjs";

const WASM = resolve("./roundhouse_wasm.wasm");
const FIXTURE = resolve("./fixture.json");
const EXPECTED_FILES = 15;

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

if (result.files.length !== EXPECTED_FILES) {
  console.error(`\nFAIL: expected ${EXPECTED_FILES} files, got ${result.files.length}`);
  process.exit(1);
}
console.log(`\nOK: ${EXPECTED_FILES} files as expected`);
