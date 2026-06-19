// Phase 5-8 smoke: drive /studio/ in chromium and assert the whole client-side
// loop — boot → transpile (worker profile) → esbuild bundle → host in a service
// worker → RUN the app in an iframe over sqlite-wasm → edit Ruby → the running
// app reflects it. Phase 6 asserts the emitted Minitest suite ships in the
// browser payload and is live (a test-source edit reaches the shipped spec);
// Phase 7 asserts it RUNS in-browser (per-file in-memory DB); Phase 8 asserts
// the results PANEL paints — green (badge/suites/cases + 9-target conformance
// strip) and red (failing rows + messages) after a broken assertion; Phase 9
// asserts a failing test maps + jumps back to its Ruby `test "..."` source line.
// Editor-widget agnostic (via window.__studio).
// (Needs network: esbuild + Monaco + sqlite-wasm/turbo + Tailwind load from CDNs.)
//
// Serve the PARENT (wasm/) as the web root:
//   python3 -m http.server 8099   # run from wasm/
//   node verify-studio.mjs        # (run from wasm/studio/)

import { createRequire } from "node:module";
const require = createRequire("/Users/rubys/git/roundhouse/tests/browser_smoke/");
const { chromium } = require("playwright");

const URL = "http://localhost:8099/studio/index.html";
const MODEL = "app/models/article.rb";
const VIEW = "app/views/articles/index.html.erb";
const MARKER = "STUDIO-LIVE-EDIT-OK";

const browser = await chromium.launch();
const page = await browser.newPage();
const logs = [];
page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));
page.on("pageerror", (e) => logs.push(`[pageerror] ${e.message}`));

let failed = false;
const fail = (msg) => { console.error(`FAIL: ${msg}`); failed = true; };
const frameText = () => page.evaluate(() => document.getElementById("appFrame")?.contentDocument?.body?.innerText || "");

await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector("#status.ok", { timeout: 30000 });
await page.waitForFunction(() => window.__studio && window.__studio.ready, { timeout: 30000 });

// --- boot: worker-profile build + esbuild bundle + app host ------------------
console.log("=== boot ===");
const initial = await page.evaluate(() => window.__studio.build());
console.log("editor:", await page.evaluate(() => window.__studio.editorKind),
  "| files:", initial.files?.length,
  "| bundler:", await page.evaluate(() => window.__studio.hasBundler()),
  "| appHost:", await page.evaluate(() => window.__studio.hasAppHost()));
if (initial.error) fail(`initial build errored: ${initial.error}`);
for (const p of ["main.ts", "worker.ts", "src/db_worker.ts", "vite.config.ts"])
  if (!initial.files?.some((f) => f.path === p)) fail(`worker profile missing ${p}`);
if (!(await page.evaluate(() => window.__studio.hasBundler()))) fail("esbuild bundler failed to load");
if (!(await page.evaluate(() => window.__studio.hasAppHost()))) fail("service-worker app host unavailable");
await page.waitForFunction(() => window.__studio.bundle()?.outputs && Object.keys(window.__studio.bundle().outputs).length === 3, { timeout: 30000 });
const bundle = await page.evaluate(() => window.__studio.bundle());
console.log("bundle:", Object.entries(bundle.outputs).map(([n, o]) => `${n} ${(o.bytes / 1024).toFixed(0)}K`).join(" "), `· ${bundle.ms?.toFixed(0)}ms`);
if (bundle.errors?.length) fail(`bundle errors: ${bundle.errors.map((e) => e.text).join("; ")}`);

// --- Phase 6: the emitted Minitest suite ships in the browser payload --------
// roundhouse transpiles the Rails Minitest suites to TS under the worker
// profile; this asserts those specs + the in-browser harness + fixtures reach
// the browser (not just CI). Running them is Phase 7-8.
console.log("\n=== test suite shipped (Phase 6) ===");
const suite = await page.evaluate(() => window.__studio.testSuite());
const specPaths = suite?.specs.map((f) => f.path) || [];
console.log("specs:", specPaths.join(", "));
console.log("runtime:", (suite?.runtime || []).map((f) => f.path).join(", "),
  "| fixtures:", (suite?.fixtures || []).map((f) => f.path).join(", "));
for (const p of ["test/article.test.ts", "test/comment.test.ts", "test/articles_controller.test.ts", "test/comments_controller.test.ts"])
  if (!specPaths.includes(p)) fail(`emitted suite missing spec ${p}`);
if (!suite?.runtime.some((f) => f.path === "test/_runtime/minitest.ts")) fail("emitted suite missing the test/_runtime/minitest.ts harness");
for (const p of ["test/fixtures/articles.ts", "test/fixtures/comments.ts"])
  if (!suite?.fixtures.some((f) => f.path === p)) fail(`emitted suite missing fixture ${p}`);

// --- Phase 7: the suite RUNS in-browser, green, on a fresh in-memory DB ------
// Bundle the emitted suite (in-memory DB + node:test/assert shims) and run it
// in a throwaway worker. The PRISTINE fixture must be all-green.
console.log("\n=== test run (Phase 7) ===");
const run = await page.evaluate(() => window.__studio.runTests());
if (run.error) {
  fail(`test run errored: ${run.error}`);
} else {
  console.log(`tests: ${run.passed}/${run.total} passed`
    + (run.failed ? `, ${run.failed} failed` : "") + (run.skipped ? `, ${run.skipped} skipped` : "")
    + ` · bundle ${(run.bundleBytes / 1024).toFixed(0)}K/${run.bundleMs?.toFixed(0)}ms`);
  for (const r of run.results.filter((x) => x.status === "fail")) console.log(`  FAIL ${r.name}: ${r.error}`);
  if (!(run.total >= 8)) fail(`expected ≥8 emitted tests, ran ${run.total}`);
  if (run.failed) fail(`${run.failed} test(s) failed on the pristine suite`);
  if (run.passed < run.total - run.skipped) fail(`not all tests passed (${run.passed}/${run.total})`);
}

// --- Phase 8: the results panel + cross-target strip render (pristine green) -
console.log("\n=== results panel (Phase 8) ===");
await page.evaluate(() => window.__studio.selectTab("tests"));
await page.evaluate(() => window.__studio.runTestsUI());
const panel = await page.evaluate(() => ({
  badge: document.getElementById("tabBadge").textContent.trim(),
  badgeOk: document.getElementById("tabBadge").classList.contains("ok"),
  suites: document.querySelectorAll("#testResults .suite").length,
  cases: document.querySelectorAll("#testResults .tcase").length,
  passes: document.querySelectorAll("#testResults .tcase.pass").length,
  chips: [...document.querySelectorAll("#conformance .chip")].map((e) => e.textContent.replace(/\s+/g, " ").trim()),
  ciChips: document.querySelectorAll("#conformance .chip:not(.live)").length,
  hasLive: !!document.querySelector("#conformance .chip.live"),
}));
console.log("badge:", panel.badge, "| suites:", panel.suites, "| cases:", panel.cases, "| pass:", panel.passes);
console.log("conformance:", panel.chips.join("  "));
if (!panel.badgeOk) fail(`tab badge not green (got "${panel.badge}")`);
if (panel.suites < 4) fail(`expected ≥4 suite groups in the panel, got ${panel.suites}`);
if (panel.cases < 8) fail(`expected ≥8 case rows in the panel, got ${panel.cases}`);
if (panel.passes !== panel.cases) fail(`panel shows non-passing rows (${panel.passes}/${panel.cases})`);
if (!panel.hasLive) fail("no live TS chip in the conformance strip");
if (panel.ciChips !== 8) fail(`expected 8 CI-attested chips, got ${panel.ciChips}`);
await page.evaluate(() => window.__studio.selectTab("app")); // back to the app for the next checks

// --- the app RUNS: iframe renders the seeded blog over sqlite-wasm ----------
console.log("\n=== running app ===");
try {
  await page.waitForFunction(() => /Getting Started with Rails/.test(document.getElementById("appFrame")?.contentDocument?.body?.innerText || ""), { timeout: 45000 });
  const t = await frameText();
  console.log("app rendered:", t.split("\n").find((l) => l.trim())?.slice(0, 40), "…",
    "| has all 3 seeds:", ["Getting Started with Rails", "Understanding MVC", "Ruby2JS"].every((s) => t.includes(s)));
  if (!["Getting Started with Rails", "Understanding MVC", "Ruby2JS"].every((s) => t.includes(s)))
    fail("running app did not render all 3 seeded articles");
} catch (e) {
  fail(`running app did not render: ${e.message}`);
  console.log("frame text:", (await frameText()).slice(0, 200));
}

// Tailwind (browser JIT) actually styles the app: the `bg-blue-600` button must
// have a real background (not transparent). Guards the reconcileHead fix that
// keeps the injected <style> from being wiped on the initial render.
const btnBg = await page.evaluate(() => {
  const el = document.getElementById("appFrame")?.contentDocument?.querySelector("a.bg-blue-600");
  return el ? getComputedStyle(el).backgroundColor : null;
});
console.log("tailwind: .bg-blue-600 background =", btnBg);
if (!btnBg || btnBg === "rgba(0, 0, 0, 0)" || btnBg === "transparent") fail("Tailwind styling not applied (bg-blue-600 transparent)");

// --- edit → reload: the running app reflects a view edit --------------------
console.log("\n=== edit → reload loop ===");
await page.evaluate((m) => {
  const orig = window.__studio.source("app/views/articles/index.html.erb");
  return window.__studio.editFile("app/views/articles/index.html.erb", `<p data-test="marker">${m}</p>\n` + orig);
}, MARKER);
try {
  await page.waitForFunction((m) => (document.getElementById("appFrame")?.contentDocument?.body?.innerText || "").includes(m), MARKER, { timeout: 45000 });
  console.log(`running app reflects the edit ("${MARKER}") ✓`);
} catch (e) {
  fail(`edit did not reach the running app: ${e.message}`);
}

// --- a model edit still moves the emitted TS (transpile is live) ------------
const edited = await page.evaluate((p) => {
  const orig = window.__studio.source(p);
  const next = orig.replace("length: { minimum: 10 }", "length: { minimum: 999 }");
  if (next === orig) return { error: "validation string not found" };
  window.__studio.editFile(p, next);
  return window.__studio.build();
}, MODEL);
if (edited.error) fail(`model edit: ${edited.error}`);
else if (!/999/.test(edited.files.find((f) => f.path === "app/models/article.ts")?.content || "")) fail("emitted model TS did not reflect the edit");
else console.log("\nmodel edit reflected in emitted TS ✓");

// --- a test-SOURCE edit flows into the shipped suite (Phase 6: the suite is
//     LIVE, not a baked CI artifact). Change an assertion literal in the Ruby
//     test → it appears in the emitted, shipped test/article.test.ts. --------
const tedit = await page.evaluate(async (p) => {
  const orig = window.__studio.source(p);
  const next = orig.replace("Getting Started with Rails", "STUDIO-PHASE6-SUITE-LIVE");
  if (next === orig) return { error: "assertion literal not found in test source" };
  await window.__studio.editFile(p, next);
  return { suite: window.__studio.testSuite() };
}, "test/models/article_test.rb");
if (tedit.error) fail(`test-source edit: ${tedit.error}`);
else if (!/STUDIO-PHASE6-SUITE-LIVE/.test(tedit.suite.specs.find((f) => f.path === "test/article.test.ts")?.content || ""))
  fail("test-source edit did not reach the emitted, shipped spec");
else console.log("test-source edit flows into the shipped suite ✓");

// --- Phase 8: a broken assertion paints the panel RED (counts + messages) ---
// The edits above broke article_test.rb's title assertion, so re-running must
// report red — a red badge, failing rows, and the failure message in-panel.
await page.evaluate(() => window.__studio.selectTab("tests"));
const redRun = await page.evaluate(() => window.__studio.runTestsUI());
const red = await page.evaluate(() => ({
  badgeErr: document.getElementById("tabBadge").classList.contains("err"),
  fails: document.querySelectorAll("#testResults .tcase.fail").length,
  msgs: document.querySelectorAll("#testResults .terr").length,
}));
console.log("red panel:", red.fails, "failing rows,", red.msgs, "messages | badge err:", red.badgeErr);
if (redRun.error) fail(`red-run errored: ${redRun.error}`);
else if (!(redRun.failed >= 1)) fail(`expected the broken assertion to fail a test, got ${redRun.failed} failure(s)`);
else if (!red.badgeErr || red.fails < 1 || red.msgs < 1) fail(`panel did not render red (rows ${red.fails}, msgs ${red.msgs}, badgeErr ${red.badgeErr})`);
else console.log(`results panel paints red (${red.fails} rows + messages) after a broken assertion ✓`);

// --- Phase 9: a failing test maps + jumps back to its Ruby source line -------
console.log("\n=== debug leg: test → Ruby source (Phase 9) ===");
// (a) the sourcemap-driven resolver lands on the right Ruby file + test line.
const loc = await page.evaluate(() =>
  window.__studio.sourceLocForTest("ArticleTest#test_creates_an_article_with_valid_attributes"));
const rubyLine = await page.evaluate((l) =>
  l && (window.__studio.source(l.path) || "").split("\n")[l.line - 1], loc);
console.log("resolved:", loc ? `${loc.path}:${loc.line}` : "(null)", "|", (rubyLine || "").trim());
if (!loc) fail("failing test did not resolve to a Ruby source location");
else if (loc.path !== "test/models/article_test.rb") fail(`wrong Ruby file: ${loc.path}`);
else if (!/test\s+["']creates an article with valid attributes["']/.test(rubyLine || ""))
  fail(`Ruby line ${loc.line} is not the test declaration: ${JSON.stringify(rubyLine)}`);

// (b) clicking a failing row opens that Ruby file in the editor.
const clicked = await page.evaluate(() => {
  const row = document.querySelector("#testResults .tcase.fail");
  if (!row) return null;
  row.click();
  return window.__studio.currentFile();
});
console.log("clicked failing row → editor file:", clicked);
if (!clicked || !/_test\.rb$/.test(clicked)) fail(`clicking a failing row did not open a Ruby test file (got ${clicked})`);
else console.log("clicking a failing test jumps to its Ruby source ✓");

await page.screenshot({ path: "studio.png" });

// Benign noise: CDN/monaco chatter, the juntos info logs, and sqlite's OPFS
// SAB probe (it tries the SAB-OPFS VFS, falls back to the no-header sahpool).
const noise = /monaco|cdn\.jsdelivr|esm\.sh|loader\.js|\[juntos\]|SharedArrayBuffer|OPFS|sqlite3_vfs|favicon/i;
const realErrors = logs.filter((l) => /pageerror|\[error\]/.test(l) && !noise.test(l));
if (realErrors.length) {
  console.log("\n=== console errors ===");
  realErrors.forEach((l) => console.log(l));
  fail(`${realErrors.length} console/page error(s)`);
}

await browser.close();
if (failed) process.exit(1);
console.log("\nOK: studio runs the emitted blog live in-browser and reflects Ruby edits (full-reload loop).");
