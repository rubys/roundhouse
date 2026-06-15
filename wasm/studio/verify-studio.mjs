// Phase 4 scaffold smoke: drive /studio/ in chromium and assert the shared
// ../lib/ works on this second surface — boot, source tree, editor, and the
// debounced edit→transpile (build) loop — plus the Phase-5-roadmap app pane.
// Editor-widget agnostic (via window.__studio), so it passes under Monaco or
// the textarea fallback.
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

// --- boot: TS-only build produces files, no error ---------------------------
console.log("=== boot ===");
const editorKind = await page.evaluate(() => window.__studio.editorKind);
const initial = await page.evaluate(() => window.__studio.build());
console.log("editor:", editorKind, "| files:", initial.files?.length,
  "| sources:", await page.evaluate(() => window.__studio.sourceCount()));
if (initial.error) fail(`initial build errored: ${initial.error}`);
if (!(initial.files?.length > 0)) fail(`expected >0 TS files, got ${initial.files?.length}`);
if (!initial.files?.some((f) => f.path.endsWith(".ts"))) fail("no .ts files emitted");

const modelFile = (b) => b.files.find((f) => f.path === "app/models/article.ts");
const before = modelFile(initial);
if (!before) fail("no app/models/article.ts in initial build");

// --- edit: a reflected validation moves the emitted TS ----------------------
const edited = await page.evaluate((p) => {
  const orig = window.__studio.source(p);
  const next = orig.replace("length: { minimum: 10 }", "length: { minimum: 999 }");
  if (next === orig) return { error: "edit precondition failed: validation string not found" };
  window.__studio.editFile(p, next);
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

// --- app pane: build readout + Phase 5 roadmap are rendered ------------------
console.log("\n=== app pane ===");
const appText = await page.evaluate(() => document.getElementById("appHost").textContent);
const hasRoadmap = /Live app loop/i.test(appText) && /Phase 5/i.test(appText);
const hasBuildline = /last build/i.test(appText) && /typescript/i.test(appText);
console.log("roadmap shown:", hasRoadmap, "| build readout shown:", hasBuildline);
if (!hasRoadmap) fail("app pane missing the Phase 5 roadmap text");
if (!hasBuildline) fail("app pane missing the live build readout");

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
