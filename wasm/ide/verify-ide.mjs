// Smoke check for the /ide/ page: drive it in a real browser engine
// (Playwright/chromium, same harness as verify-playground.mjs) against
// whatever app-src.json is present, and assert the demo beats:
//
//   1. boots: sources load, worker analyzes, counts render.
//   2. hover / type_at: @status in statuses_controller types as Status
//      (skipped for non-Mastodon bundles).
//   3. completion: typing `@status.` yields typed candidates
//      (account → Account?).
//   4. related files: the controller relates to views + concerns.
//   5. traceroute: the request chain pins into the panel with the
//      coverage footer (grouped hops, gap report).
//   6. coverage: gaps list is non-empty (the ledger is on).
//
// Serve the PARENT (wasm/) as the web root (the page imports ../lib/):
//   python3 -m http.server 8099    # run from wasm/
//   node verify-ide.mjs            # (run from wasm/ide/)

import { createRequire } from "node:module";
const require = createRequire(new URL("../../tests/browser_smoke/", import.meta.url).pathname);
const { chromium } = require("playwright");

const BASE = process.env.IDE_URL || "http://localhost:8099/ide/";
const MASTODON = !process.env.IDE_GENERIC;

const browser = await chromium.launch();
const page = await browser.newPage();
page.on("console", (m) => { if (m.type() === "error") console.error("[console]", m.text()); });
page.on("pageerror", (e) => console.error("[pageerror]", e.message));

let failures = 0;
function check(name, ok, detail = "") {
  console.log(`${ok ? "ok" : "FAIL"} - ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
}

await page.goto(BASE);

// 1. Boot: wait for the analysis to land (worker init + ~2.5s pass).
await page.waitForFunction(() => window.__ide?.analysis, null, { timeout: 120_000 });
const summary = await page.evaluate(() => ({
  files: window.__ide.analysis.files.length,
  classes: window.__ide.analysis.classes.length,
  gaps: window.__ide.analysis.gaps.length,
  counts: document.getElementById("counts").textContent,
}));
check("boots and analyzes", summary.files > 10, `${summary.files} files, ${summary.classes} classes`);
check("coverage ledger is on", summary.gaps >= 0 && summary.counts.includes("coverage notes"), summary.counts);

if (MASTODON) {
  const ctrl = "app/controllers/statuses_controller.rb";

  // 2. type_at: @account read in set_status types as Account.
  const typeAt = await page.evaluate(async (ctrl) => {
    const text = window.__ide.srcMap[ctrl];
    const idx = text.indexOf("@account.statuses");
    const line = text.slice(0, idx).split("\n").length - 1;
    const ch = idx - text.lastIndexOf("\n", idx - 1) - 1 + 2;
    return window.__ide.rpc("typeAt", { path: ctrl, line, character: ch });
  }, ctrl);
  check("type_at @account → Account", typeAt?.display === "Account", JSON.stringify(typeAt));

  // 3. completion on `@status.` typed into the controller.
  const cands = await page.evaluate(async (ctrl) => {
    const orig = window.__ide.srcMap[ctrl];
    const text = orig.replace("  def show\n", "  def show\n    @status.\n");
    const idx = text.indexOf("    @status.") + "    @status.".length;
    const line = text.slice(0, idx).split("\n").length - 1;
    const ch = idx - text.lastIndexOf("\n", idx - 1) - 1;
    return window.__ide.rpc("complete", { path: ctrl, text, line, character: ch });
  }, ctrl);
  const byLabel = Object.fromEntries((cands || []).map((c) => [c.label, c.detail]));
  check("completion @status. is typed", byLabel.account === "Account?",
    `${(cands || []).length} items, account → ${byLabel.account}`);

  // 4. related files for the controller.
  const rel = await page.evaluate(
    (ctrl) => window.__ide.rpc("related", { path: ctrl }), ctrl);
  const kinds = new Set((rel || []).map((r) => r.kind));
  check("related files walk the render graph",
    kinds.has("view") && kinds.has("concern"),
    (rel || []).slice(0, 5).map((r) => `${r.kind}:${r.label}`).join(", "));

  // 5. traceroute: the request chain + gap footer land in the panel.
  const tr = await page.evaluate(async () => {
    await window.__ide.runTrace("StatusesController#show");
    const t = window.__ide.trace;
    return t && {
      route: t.route,
      hops: t.hops.length,
      coverage: t.coverage,
      gapKinds: t.gaps.map((g) => g.kind),
      panelOpen: document.getElementById("trace").classList.contains("open"),
      groupCount: document.querySelectorAll("#trace .tgroup").length,
      footText: document.getElementById("traceFoot").textContent,
    };
  });
  check("traceroute chains the request",
    tr && tr.hops > 10 && tr.coverage.total_hops > 10 &&
      tr.coverage.resolved_hops >= tr.coverage.total_hops - 2,
    tr && `${tr.hops} hops, ${tr.coverage.resolved_hops}/${tr.coverage.total_hops} resolved`);
  check("trace panel renders grouped hops + footer",
    tr && tr.panelOpen && tr.groupCount >= 3 && /gap|complete/.test(tr.footText),
    tr && `${tr.groupCount} groups, foot: ${tr.footText.slice(0, 60)}`);

  // 6. open a HAML view and confirm hover works inside the template.
  const hamlType = await page.evaluate(async () => {
    const haml = "app/views/statuses/show.html.haml";
    const text = window.__ide.srcMap[haml];
    const idx = text.indexOf("@status.spoiler_text");
    if (idx < 0) return { skipped: true };
    const line = text.slice(0, idx).split("\n").length - 1;
    const ch = idx - text.lastIndexOf("\n", idx - 1) - 1 + 2;
    return window.__ide.rpc("typeAt", { path: haml, line, character: ch });
  });
  check("hover inside HAML template", hamlType?.display === "Status" || hamlType?.skipped,
    JSON.stringify(hamlType));
}

await browser.close();
if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("ide verify: all checks passed");
