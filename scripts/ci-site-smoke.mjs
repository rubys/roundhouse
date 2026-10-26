#!/usr/bin/env node
// Check the actual assembled site, not the source directories. Existing
// surface checks run only when their browser or compiler inputs changed.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { parseArgs } from "node:util";
import { loadEngine } from "../wasm/lib/engine.mjs";

const { values } = parseArgs({ options: { root: { type: "string" }, state: { type: "string" } } });
assert(values.root && values.state, "--root and --state are required");
const root = resolve(values.root);
const state = JSON.parse(await readFile(values.state, "utf8"));
const wasm = await readFile(resolve(root, "lib/roundhouse_wasm.wasm"));
const version = (await loadEngine(wasm)).callExport("version", {});
assert.equal(version.commit, state.wasm_source_sha, "the shipped compiler must match its recorded source");

const require = createRequire(new URL("../tests/browser_smoke/", import.meta.url).pathname);
const { chromium } = require("playwright");
const browser = await chromium.launch();
try {
  for (const path of state.pages) {
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    page.on("console", message => { if (message.type() === "error") errors.push(message.text()); });
    const response = await page.goto(`http://127.0.0.1:8099/${path.split("/").map(encodeURIComponent).join("/")}`, { waitUntil: "networkidle" });
    assert(response?.ok(), `${path}: HTTP ${response?.status()}`);
    assert.deepEqual(errors, [], `${path}: browser errors`);
    console.log(`OK: ${path}`);
    await page.close();
  }
} finally {
  await browser.close();
}

for (const surface of state.browser) {
  assert(["playground", "studio", "ide", "fractal"].includes(surface), "unknown browser surface");
  const script = new URL(`../wasm/${surface}/verify-${surface}.mjs`, import.meta.url);
  const result = spawnSync(process.execPath, [script.pathname], { stdio: "inherit" });
  assert.equal(result.status, 0, `${surface}: browser verification failed`);
}
console.log(`OK: compiler ${version.commit}, ${state.pages.length} site page(s), and ${state.browser.length} affected browser surface(s)`);
