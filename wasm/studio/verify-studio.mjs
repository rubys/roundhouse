// Phase 4 smoke: drive /studio/ in chromium and assert the shared ../lib/ on
// this second surface — boot, source tree, editor, the debounced
// edit→transpile (worker profile) loop — AND the esbuild-wasm bundle step that
// turns the emitted TS into browser-loadable ESM. Editor-widget agnostic (via
// window.__studio), so it passes under Monaco or the textarea fallback.
// (Needs network: esbuild + Monaco load from a CDN.)
//
// Serve the PARENT (wasm/) as the web root:
//   python3 -m http.server 8099   # run from wasm/
//   node verify-studio.mjs        # (run from wasm/studio/)

import { createRequire } from "node:module";
const require = createRequire("/Users/rubys/git/roundhouse/tests/browser_smoke/");
const { chromium } = require("playwright");

const URL = "http://localhost:8099/studio/index.html";
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
await page.waitForFunction(() => window.__studio && window.__studio.ready, { timeout: 30000 });

// --- boot: worker-profile build produces the runnable app -------------------
console.log("=== boot ===");
const editorKind = await page.evaluate(() => window.__studio.editorKind);
const initial = await page.evaluate(() => window.__studio.build());
console.log("editor:", editorKind, "| files:", initial.files?.length,
  "| sources:", await page.evaluate(() => window.__studio.sourceCount()));
if (initial.error) fail(`initial build errored: ${initial.error}`);
if (!(initial.files?.length > 0)) fail(`expected >0 TS files, got ${initial.files?.length}`);
// worker profile (not default): the SharedWorker app entries must be present.
for (const p of ["main.ts", "worker.ts", "src/db_worker.ts", "vite.config.ts"])
  if (!initial.files?.some((f) => f.path === p)) fail(`worker profile missing ${p}`);

// --- esbuild bundle: emitted TS → 3 browser-loadable ESM bundles -------------
console.log("\n=== esbuild bundle ===");
const hasBundler = await page.evaluate(() => window.__studio.hasBundler());
console.log("bundler loaded:", hasBundler);
if (!hasBundler) fail("esbuild-wasm bundler failed to load (network/CDN?)");
// build() kicks the bundle async; wait for it to land.
await page.waitForFunction(() => window.__studio.bundle() != null, { timeout: 30000 });
const bundle = await page.evaluate(() => window.__studio.bundle());
const outNames = Object.keys(bundle.outputs || {});
console.log("bundle:", bundle.ms?.toFixed(0), "ms |", bundle.errors?.length, "errors | outputs:", outNames.join(", "));
console.log("sizes:", outNames.map((n) => `${n} ${(bundle.outputs[n].bytes / 1024).toFixed(1)}KB`).join(" · "));
if (bundle.errors?.length) fail(`bundle errors: ${bundle.errors.map((e) => e.text).join("; ")}`);
for (const n of ["main.js", "worker.js", "db_worker.js"])
  if (!outNames.includes(n)) fail(`bundle missing ${n}`);
// externals must survive as full CDN URLs (worker-safe — no importmap in workers)
if (!/esm\.sh|cdn\.jsdelivr/.test(bundle.outputs["main.js"].text)) fail("main bundle has no CDN-external import");

const modelFile = (b) => b.files.find((f) => f.path === "app/models/article.ts");
const before = modelFile(initial);
if (!before) fail("no app/models/article.ts in initial build");

// --- edit: a reflected validation moves the emitted TS ----------------------
const edited = await page.evaluate(async (p) => {
  const orig = window.__studio.source(p);
  const next = orig.replace("length: { minimum: 10 }", "length: { minimum: 999 }");
  if (next === orig) return { error: "edit precondition failed: validation string not found" };
  await window.__studio.editFile(p, next);
  return window.__studio.build();
}, MODEL);
console.log("\n=== after edit (minimum 10 -> 999) ===");
if (edited.error) {
  fail(`build errored after edit: ${edited.error}`);
} else {
  const after = modelFile(edited);
  const changed = before && after && after.content !== before.content;
  const reflects = after && /999/.test(after.content);
  console.log("model file len", before?.content.length, "->", after?.content.length,
    "| reflects 999:", reflects, "| changed:", changed);
  if (!changed) fail("emitted model TS did not change after editing the source");
  if (!reflects) fail("emitted model TS does not reflect the changed validation");
}

// --- app pane: transpile + bundle readouts + Phase 5 roadmap ----------------
console.log("\n=== app pane ===");
const appText = await page.evaluate(() => document.getElementById("appHost").textContent);
const hasRoadmap = /Live app loop/i.test(appText) && /Phase 5/i.test(appText);
const hasTranspile = /transpile/i.test(appText) && /typescript/i.test(appText);
const hasBundle = /bundle/i.test(appText) && /ready to run/i.test(appText);
console.log("roadmap:", hasRoadmap, "| transpile readout:", hasTranspile, "| bundle readout:", hasBundle);
if (!hasRoadmap) fail("app pane missing the Phase 5 roadmap text");
if (!hasTranspile) fail("app pane missing the live transpile readout");
if (!hasBundle) fail("app pane missing the live bundle readout");

await page.screenshot({ path: "studio.png" });

const noise = /monaco|web worker|cdn\.jsdelivr|loader\.js/i;
const realErrors = logs.filter((l) => /pageerror|\[error\]/.test(l) && !noise.test(l));
if (realErrors.length) {
  console.log("\n=== console errors ===");
  realErrors.forEach((l) => console.log(l));
  fail(`${realErrors.length} console/page error(s)`);
}

await browser.close();

if (failed) process.exit(1);
console.log("\nOK: studio scaffold — shared lib editor + source tree + edit→transpile loop verified in a real browser tab.");
