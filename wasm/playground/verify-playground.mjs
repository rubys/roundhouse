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
//   4. diagnostics overlay: baseline warnings are present, and a type-error
//      edit (`title + 1`) surfaces an incompatible_binop error rendered as a
//      Monaco squiggle.
//   5. inferred-type hovers: `title` in the edited method types as String, and
//      the result carries many inferred types.
//
// (Note: a plain `def foo` method is NOT carried into the model emit today —
// the transpiler reflects recognized Rails DSL like `validates`, not arbitrary
// methods — so the edit assertion uses a validation, which IS reflected.)
//
// Serve THIS directory as the web root (it's self-contained):
//   python3 -m http.server 8099    # run from wasm/playground/
//   node verify-playground.mjs

import { createRequire } from "node:module";
const require = createRequire("/Users/rubys/git/roundhouse/tests/browser_smoke/");
const { chromium } = require("playwright");

const URL = "http://localhost:8099/index.html";
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

// Boot defaults: ruby is the initial target, and the dropdown is alphabetical.
console.log("=== boot defaults ===");
const defaultTarget = await page.evaluate(() => document.getElementById("target").value);
const optionOrder = await page.evaluate(() =>
  [...document.querySelectorAll("#target option")].map((o) => o.value));
console.log("default target:", defaultTarget, "| options:", optionOrder.join(", "));
if (defaultTarget !== "ruby") fail(`expected default target ruby, got ${defaultTarget}`);
if (optionOrder.join() !== [...optionOrder].sort().join())
  fail(`target options not alphabetical: ${optionOrder.join(", ")}`);
for (const t of ["kotlin", "swift", "ruby"])
  if (!optionOrder.includes(t)) fail(`missing newly-wired target ${t}`);
const rubyBoot = await page.evaluate(() => window.__playground.output());
if (rubyBoot.error) fail(`ruby default transpile errored: ${rubyBoot.error}`);
if (!rubyBoot.files?.some((f) => f.path.endsWith(".rb"))) fail("ruby default emitted no .rb files");

// Switch to typescript for the TS-shaped assertions that follow.
await page.evaluate(() => window.__playground.setTarget("typescript"));

const editorKind = await page.evaluate(() => window.__playground.editorKind);
const initial = await page.evaluate(() => window.__playground.output());
console.log("\n=== boot ===");
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
for (const t of ["typescript", "go", "rust", "python", "elixir", "crystal", "kotlin", "swift", "ruby"]) {
  const out = await page.evaluate((target) => {
    window.__playground.setTarget(target);
    return window.__playground.output();
  }, t);
  const count = out.files?.length ?? 0;
  const err = out.error || count < 1;
  console.log(`${t}: ${out.error ? `ERROR — ${out.error}` : `${count} files`}`);
  if (err) fail(`${t} produced no output`);
}

// --- diagnostics overlay: baseline warnings + edit introduces an error ------
console.log("\n=== diagnostics (inference overlay) ===");
await page.evaluate(() => window.__playground.setTarget("typescript"));
const baseDiag = await page.evaluate(() => window.__playground.diagnostics());
console.log("baseline:", baseDiag.length, "—", [...new Set(baseDiag.map((d) => d.code))].join(", "));
if (!baseDiag.some((d) => d.severity === "warning")) fail("expected baseline warnings (gradual_untyped)");

const errDiag = await page.evaluate((p) => {
  const orig = window.__playground.source(p);
  const next = orig.replace("class Article < ApplicationRecord\n",
    "class Article < ApplicationRecord\n  def bad\n    title + 1\n  end\n\n");
  window.__playground.editFile(p, next);
  return window.__playground.diagnostics();
}, MODEL);
const typeErr = errDiag.find((d) =>
  d.severity === "error" && d.code === "incompatible_binop" && d.path === MODEL);
console.log("after `title + 1` edit:", errDiag.length,
  "| incompatible_binop error:", typeErr ? `@${typeErr.start_line}:${typeErr.start_col}` : "MISSING");
if (!typeErr) fail("expected an incompatible_binop error after the type-error edit");

// confirm the squiggle actually rendered in Monaco (not just plumbed as data)
const errorMarkers = await page.evaluate(() => {
  if (!window.monaco) return -1; // textarea fallback — no markers
  return window.monaco.editor.getModelMarkers({ owner: "roundhouse" })
    .filter((m) => m.severity === window.monaco.MarkerSeverity.Error).length;
});
console.log("monaco error markers on open file:", errorMarkers < 0 ? "(textarea fallback)" : errorMarkers);
if (errorMarkers === 0) fail("expected an error squiggle rendered in Monaco");

// --- inferred-type hovers: `title` in the edited method types as String -----
console.log("\n=== inferred-type hovers ===");
const titleType = await page.evaluate(() => window.__playground.typeAt(3, 6));
const typeCount = await page.evaluate(() => window.__playground.types().length);
console.log("type at article.rb:3:6 (`title`):", titleType, "| total inferred types:", typeCount);
if (titleType !== "String") fail(`expected String at the \`title\` position, got ${titleType}`);
if (typeCount < 100) fail(`expected many inferred types, got ${typeCount}`);

// --- source -> output follow: selecting a source shows its emitted file ------
// Heuristic name match (no `source` field on EmittedFile yet): basename, then
// tighten by parent dir until unique. Covers the TS exact-path case and rust's
// app/ -> src/ prefix divergence.
console.log("\n=== source → output follow ===");
await page.evaluate(() => window.__playground.setTarget("typescript"));
await page.evaluate(() => window.__playground.selectSource("app/models/article.rb"));
const tsModelOut = await page.evaluate(() => window.__playground.displayedOutput());
console.log("ts: select app/models/article.rb ->", tsModelOut);
if (tsModelOut !== "app/models/article.ts") fail(`expected app/models/article.ts, got ${tsModelOut}`);

await page.evaluate(() => window.__playground.selectSource("app/views/articles/index.html.erb"));
const tsViewOut = await page.evaluate(() => window.__playground.displayedOutput());
console.log("ts: select app/views/articles/index.html.erb ->", tsViewOut);
if (tsViewOut !== "app/views/articles/index.ts") fail(`expected app/views/articles/index.ts, got ${tsViewOut}`);

// rust relocates app/ -> src/; the suffix-walk should still map controllers.
await page.evaluate(() => window.__playground.setTarget("rust"));
await page.evaluate(() => window.__playground.selectSource("app/controllers/application_controller.rb"));
const rsCtrlOut = await page.evaluate(() => window.__playground.displayedOutput());
console.log("rust: select app/controllers/application_controller.rb ->", rsCtrlOut);
if (!/controllers\/application_controller\.rs$/.test(rsCtrlOut || ""))
  fail(`expected a rust controllers/application_controller.rs, got ${rsCtrlOut}`);

// ruby: source .rb and output .rb share a name, and an .rbs sidecar sits beside
// it — the sidecar exclusion must keep the follow landing on the .rb (not no-op).
await page.evaluate(() => window.__playground.setTarget("ruby"));
await page.evaluate(() => window.__playground.selectSource("app/models/article.rb"));
const rbOut = await page.evaluate(() => window.__playground.displayedOutput());
console.log("ruby: select app/models/article.rb ->", rbOut);
if (rbOut !== "app/models/article.rb") fail(`expected app/models/article.rb, got ${rbOut}`);

await page.evaluate(() => { window.__playground.setTarget("typescript"); window.__playground.selectSource("app/models/article.rb"); });
await page.screenshot({ path: "playground.png" });

const noise = /monaco|web worker|cdn\.jsdelivr|loader\.js/i;
const realErrors = logs.filter((l) => /pageerror|\[error\]/.test(l) && !noise.test(l));
if (realErrors.length) {
  console.log("\n=== console errors ===");
  realErrors.forEach((l) => console.log(l));
  fail(`${realErrors.length} console/page error(s)`);
}

await browser.close();

if (failed) process.exit(1);
console.log("\nOK: edit -> transpile -> render loop + diagnostics overlay + inferred-type hovers verified in a real browser tab.");
