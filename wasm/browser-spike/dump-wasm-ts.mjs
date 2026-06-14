// Dump the wasm transpile output to a directory for byte-comparison
// against the native `emit_preview` reference.
// Usage: node dump-wasm-ts.mjs <wasm> <out-dir>
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { loadCompiler } from "./transpile.mjs";

const WASM = process.argv[2] || "./roundhouse_wasm.wasm";
const OUT = process.argv[3] || "/tmp/rh-wasm-ts";

const fixture = JSON.parse(await readFile("./fixture.json", "utf8"));
const compiler = await loadCompiler(await readFile(WASM));
const result = compiler.transpile("typescript", fixture);
if (result.error) {
  console.error("ERROR:", result.error);
  process.exit(1);
}
for (const f of result.files) {
  const p = join(OUT, f.path);
  await mkdir(dirname(p), { recursive: true });
  await writeFile(p, f.content);
}
console.log(`wrote ${result.files.length} files to ${OUT}`);
