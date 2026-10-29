// Smoke check for the Fractal pass explorer: drive /fractal/ in a real
// browser engine (Playwright/chromium) and assert the edit -> passes ->
// render loop over the compiler wasm's `fractal_stages` export.
//
// Asserts:
//   1. boots and compiles app/models/article.rb: the source, then
//      inline-delegate, which writes out the simple delegation (two methods) and
//      declines the prefixed one;
//   2. editing the model re-runs the passes and shows the edited source;
//   3. no console or page errors.
//
// Serve the PARENT (wasm/) as the web root, since the page imports ../lib/:
//   python3 -m http.server 8099    # run from wasm/
//   node verify-fractal.mjs        # (run from wasm/fractal/)

import { createRequire } from "node:module";
const require = createRequire(new URL("../../tests/browser_smoke/", import.meta.url).pathname);
const { chromium } = require("playwright");

const PAGE_URL = "http://localhost:8099/fractal/index.html";

const browser = await chromium.launch();
const page = await browser.newPage();
const logs = [];
page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));
page.on("pageerror", (e) => logs.push(`[pageerror] ${e.message}`));
page.on("crash", () => logs.push("[crash] the page's renderer process died (OOM?)"));

let failed = false;
const fail = (msg) => { console.error(`FAIL: ${msg}`); failed = true; };
const bail = (label) => (err) => {
  console.error(`\n=== ${label} — last 30 page log lines ===`);
  logs.slice(-30).forEach((l) => console.error(l));
  console.error(`\n=== ${label} ===`);
  console.error(err);
  process.exit(1);
};
process.on("uncaughtException", bail("uncaught exception"));
process.on("unhandledRejection", bail("unhandled rejection"));

const state = () => page.evaluate(() => ({
  status: document.getElementById("status").textContent,
  chips: [...document.querySelectorAll(".chip .name")].map((e) => e.textContent),
  lines: [...document.querySelectorAll("#code .line .txt")].map((e) => e.textContent),
  file: document.getElementById("file").value,
}));

await page.goto(PAGE_URL, { waitUntil: "load" });
await page.waitForSelector("#status.ok", { timeout: 60000 });

console.log("=== boot ===");
let s = await state();
console.log(s.status, "|", s.chips.join(" → "), "|", s.file);
if (s.file !== "app/models/article.rb") fail(`expected article.rb to be compiled, got ${s.file}`);
if (s.chips.join(",") !== "source,inline-delegate") fail(`expected source → inline-delegate, got: ${s.chips.join(", ")}`);
if (!/^1 rewrite/.test(s.status)) fail(`expected 1 rewrite, got "${s.status}"`);
if (!s.lines.some((t) => t.includes("def name(...);"))) fail(`delegation not rewritten: ${JSON.stringify(s.lines)}`);
if (!s.lines.some((t) => t.includes("delegate :time_zone, to: :author, prefix: true"))) fail(`prefixed delegation should stay: ${JSON.stringify(s.lines)}`);
// The notes render a reason's `code` spans as <code>, so textContent has no backticks.
const notes = await page.evaluate(() => document.getElementById("notes").textContent);
if (!notes.includes("passes prefix:; only to: alone is rewritten")) fail(`expected the prefix decline in the notes: ${notes}`);

console.log("=== edit app/models/article.rb ===");
await page.evaluate(() => {
  const e = document.getElementById("editor");
  e.value = e.value.replace("published? && !archived?", "published? && !draft?");
  e.dispatchEvent(new Event("input"));
});
await page.waitForFunction(
  () => [...document.querySelectorAll("#code .line .txt")].some((e) => e.textContent.includes("published? && !draft?")),
  { timeout: 30000 },
);
s = await state();
console.log(s.status, "|", s.chips.join(" → "));
if (s.chips.join(",") !== "source,inline-delegate" || !/^1 rewrite/.test(s.status)) fail(`unexpected re-run: ${s.status} | ${s.chips.join(", ")}`);

const realErrors = logs.filter((l) => /pageerror|\[error\]|\[crash\]/.test(l));
if (realErrors.length) {
  console.log("\n=== console errors ===");
  realErrors.forEach((l) => console.log(l));
  fail(`${realErrors.length} console/page error(s)`);
}

await browser.close();

if (failed) process.exit(1);
console.log("\nOK: Fractal pass explorer — inline-delegate stage and re-run on edit verified in a real browser tab.");
