// Phase 1 smoke check: drive the playground in a real browser engine
// (Playwright/chromium) and assert the full edit -> transpile -> render loop,
// editor-widget agnostic (via window.__playground, so it passes whether Monaco
// or the textarea fallback is active).
//
// Asserts:
//   1. boots + first transpile renders a positive, stable set of TS files.
//   2. editing app/models/article.rb (a validation the transpiler reflects:
//      length minimum 10 -> 999) re-transpiles and the emitted model TS
//      changes to match.
//   3. switching target re-transpiles every backend with no error.
//
// (Note: a plain `def foo` method is NOT carried into the model emit today —
// the transpiler reflects recognized Rails DSL like `validates`, not arbitrary
// methods — so the edit assertion uses a validation, which IS reflected.)
//
// Serve the wasm/ directory (NOT this dir) so ../browser-spike/ resolves:
//   python3 -m http.server 8099    # run from wasm/
//   node playground/verify-playground.mjs

import { createRequire } from "node:module";
const require = createRequire("/Users/rubys/git/roundhouse/tests/browser_smoke/");
const { chromium } = require("playwright");

const URL = "http://localhost:8099/playground/index.html";
const MODEL = "app/models/article.rb";

const browser = await chromium.launch();
const page = await browser.newPage();
const logs = [];
page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));
page.on("pageerror", (e) => logs.push(`[pageerror] ${e.message}`));

let failed = false;
const fail = (msg) => { console.error(`FAIL: ${msg}`); failed = true; };

await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector("#status.ok", { timeout: 30000 });
await page.waitForFunction(() => window.__playground && window.__playground.ready, { timeout: 30000 });

const editorKind = await page.evaluate(() => window.__playground.editorKind);
const initial = await page.evaluate(() => window.__playground.output());
console.log("=== boot ===");
console.log("editor:", editorKind);
console.log("target: typescript");
console.log("files:", initial.files?.length, "| sources:", await page.evaluate(() => window.__playground.sourceCount()));

if (initial.error) fail(`initial transpile errored: ${initial.error}`);
if (!(initial.files?.length > 0)) fail(`expected >0 TS files, got ${initial.files?.length}`);

const modelPath = (o) => o.files.find((f) => f.path === "app/models/article.ts");
const before = modelPath(initial);
if (!before) fail("no emitted app/models/article.ts in initial output");

// --- edit: change a validation the transpiler reflects, expect output to move
const edited = await page.evaluate((p) => {
  const orig = window.__playground.source(p);
  const next = orig.replace("length: { minimum: 10 }", "length: { minimum: 999 }");
  if (next === orig) return { error: "edit precondition failed: validation string not found in source" };
  window.__playground.editFile(p, next);
  return window.__playground.output();
}, MODEL);

console.log("\n=== after edit (validation minimum 10 -> 999) ===");
if (edited.error) {
  fail(`transpile errored after edit: ${edited.error}`);
} else {
  const after = modelPath(edited);
  const changed = before && after && after.content !== before.content;
  const reflects = after && /999/.test(after.content);
  console.log("model file:", after?.path, "| len", before?.content.length, "->", after?.content.length);
  console.log("reflects 999:", reflects, "| content changed:", changed);
  if (!changed) fail("emitted model TS did not change after editing the source");
  if (!reflects) fail("emitted model TS does not reflect the changed validation");
}

// --- target sweep: every backend re-transpiles cleanly ----------------------
console.log("\n=== target sweep (live re-transpile) ===");
for (const t of ["typescript", "go", "rust", "python", "elixir", "crystal"]) {
  const out = await page.evaluate((target) => {
    window.__playground.setTarget(target);
    return window.__playground.output();
  }, t);
  const count = out.files?.length ?? 0;
  const err = out.error || count < 1;
  console.log(`${t}: ${out.error ? `ERROR — ${out.error}` : `${count} files`}`);
  if (err) fail(`${t} produced no output`);
}

await page.screenshot({ path: "playground/playground.png" });

const noise = /monaco|web worker|cdn\.jsdelivr|loader\.js/i;
const realErrors = logs.filter((l) => /pageerror|\[error\]/.test(l) && !noise.test(l));
if (realErrors.length) {
  console.log("\n=== console errors ===");
  realErrors.forEach((l) => console.log(l));
  fail(`${realErrors.length} console/page error(s)`);
}

await browser.close();

if (failed) process.exit(1);
console.log("\nOK: playground edit -> transpile -> render loop verified in a real browser tab.");
