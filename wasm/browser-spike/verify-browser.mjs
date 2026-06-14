// Phase 0 exit-criterion check: drive the static spike page in a real browser
// engine (Playwright/chromium), confirm the wasm loads, transpiles, and renders
// the emitted files — and that the target dropdown re-transpiles live. Captures
// cold-load + per-target timings and a screenshot as evidence.
//
// Assumes a static server is serving this dir at http://localhost:8099.
// Resolves playwright from the repo's tests/browser_smoke/node_modules.

import { createRequire } from "node:module";
const require = createRequire("/Users/rubys/git/roundhouse/tests/browser_smoke/");
const { chromium } = require("playwright");

const URL = "http://localhost:8099/index.html";

const browser = await chromium.launch();
const page = await browser.newPage();
const logs = [];
page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));
page.on("pageerror", (e) => logs.push(`[pageerror] ${e.message}`));

await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector("#status.ok", { timeout: 20000 });

const tsStatus = await page.textContent("#status");
const tsCount = await page.locator("#files button").count();
const firstFile = await page.locator("#files button").first().textContent();
const codeLen = (await page.textContent("#code")).length;
await page.screenshot({ path: "spike-typescript.png" });

console.log("=== TypeScript (default) ===");
console.log("status:", tsStatus);
console.log("file count:", tsCount);
console.log("first file:", firstFile.trim());
console.log("rendered code length:", codeLen);

const others = [];
for (const lang of ["rust", "python", "go", "elixir", "crystal"]) {
  await page.selectOption("#target", lang);
  await page.waitForFunction(
    (l) => document.getElementById("status").textContent.startsWith(l),
    lang, { timeout: 20000 });
  const status = await page.textContent("#status");
  const count = await page.locator("#files button").count();
  const isErr = await page.locator("#status.err").count();
  others.push({ lang, status, count, error: isErr > 0 });
}

console.log("\n=== other targets (live re-transpile) ===");
for (const o of others) {
  console.log(`${o.lang}: ${o.error ? "ERROR — " : ""}${o.count} files | ${o.status}`);
}

console.log("\n=== browser console ===");
logs.forEach((l) => console.log(l));

await browser.close();

if (tsCount !== 15) {
  console.error(`\nFAIL: expected 15 TypeScript files, got ${tsCount}`);
  process.exit(1);
}
if (codeLen < 1) {
  console.error("\nFAIL: code pane empty");
  process.exit(1);
}
console.log("\nOK: roundhouse wasm transpiled real-blog in a real browser tab.");
