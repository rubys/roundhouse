// Phase 5 smoke: drive /studio/ in chromium and assert the whole client-side
// loop — boot → transpile (worker profile) → esbuild bundle → host in a service
// worker → RUN the app in an iframe over sqlite-wasm → edit Ruby → the running
// app reflects it. Editor-widget agnostic (via window.__studio).
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
