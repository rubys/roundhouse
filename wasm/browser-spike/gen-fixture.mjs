// Generate a lean fixture.json ({ path: content }) from the real-blog
// fixture for the browser spike to fetch. Skips heavy/binary dirs (tmp/
// is 2250 Rails cache files) and binary file extensions — none of which
// the transpiler reads — so the JSON stays small enough to ship statically.
//
// Run from this directory: node gen-fixture.mjs

import { readFile, readdir, writeFile } from "node:fs/promises";
import { resolve, relative, join, extname } from "node:path";

const FIXTURE = resolve("../../fixtures/real-blog");
const OUT = resolve("./fixture.json");

const SKIP_DIRS = new Set([
  "tmp", "log", "storage", ".git", "node_modules",
]);
const SKIP_EXT = new Set([
  ".sqlite3", ".png", ".ico", ".svg", ".enc", ".key",
  ".woff", ".woff2", ".ttf", ".gz", ".zip", ".jpg", ".jpeg",
]);

const src = {};
async function walk(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  for (const e of entries) {
    const full = join(dir, e.name);
    if (e.isDirectory()) {
      if (SKIP_DIRS.has(e.name)) continue;
      await walk(full);
    } else if (e.isFile()) {
      if (SKIP_EXT.has(extname(e.name))) continue;
      const rel = relative(FIXTURE, full);
      src[rel] = await readFile(full, "utf8");
    }
  }
}

await walk(FIXTURE);
const json = JSON.stringify(src);
await writeFile(OUT, json);

const bytes = Buffer.byteLength(json);
console.log(`wrote ${OUT}`);
console.log(`  ${Object.keys(src).length} files, ${(bytes / 1024).toFixed(1)} KB`);
