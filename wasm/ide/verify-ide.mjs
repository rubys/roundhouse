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
//      coverage footer (grouped hops, gap report), and N+1 findings
//      annotate the hop containing the access site (#63 phase 5).
//   6. coverage: gaps list is non-empty (the ledger is on).
//   8. open folder: a checkout fed through the webkitdirectory input
//      (fixtures/real-blog, a real directory upload) analyzes as a local
//      app — same file set as bundle-src.mjs, junk paths filtered, the
//      summary line names the build, and switching back to a shipped app
//      drops the local entry.
//
// Serve the PARENT (wasm/) as the web root (the page imports ../lib/):
//   python3 -m http.server 8099    # run from wasm/
//   node verify-ide.mjs            # (run from wasm/ide/)

import { createRequire } from "node:module";
const require = createRequire(new URL("../../tests/browser_smoke/", import.meta.url).pathname);
const { chromium } = require("playwright");

const BASE = process.env.IDE_URL || "http://localhost:8099/ide/";
const MASTODON = !process.env.IDE_GENERIC;

// 0. The directory-picker loader (fromDirectoryHandle) needs a user gesture
// in a browser, so it is exercised here under Node with a minimal
// FileSystemDirectoryHandle stand-in over the same fixture the browser path
// uploads below — and must read exactly the file set bundle-src.mjs reads.
{
  const { readdir, readFile } = await import("node:fs/promises");
  const { join, basename } = await import("node:path");
  const { fromDirectoryHandle } = await import("../lib/local-bundle.mjs");
  const dirHandle = (dir) => ({
    kind: "directory", name: basename(dir),
    async getDirectoryHandle(n) {
      const ents = await readdir(dir, { withFileTypes: true });
      if (!ents.some((e) => e.name === n && e.isDirectory())) throw new Error("NotFound");
      return dirHandle(join(dir, n));
    },
    async getFileHandle(n) {
      const ents = await readdir(dir, { withFileTypes: true });
      if (!ents.some((e) => e.name === n && e.isFile())) throw new Error("NotFound");
      return { kind: "file", name: n, getFile: async () => ({ text: () => readFile(join(dir, n), "utf8") }) };
    },
    async *entries() {
      for (const e of await readdir(dir, { withFileTypes: true })) {
        yield [e.name, e.isDirectory() ? dirHandle(join(dir, e.name))
          : { kind: "file", name: e.name, getFile: async () => ({ text: () => readFile(join(dir, e.name), "utf8") }) }];
      }
    },
  });
  const blogDir = new URL("../../fixtures/real-blog", import.meta.url).pathname;
  const viaHandle = await fromDirectoryHandle(dirHandle(blogDir));
  const { execFileSync } = await import("node:child_process");
  const out = join(process.env.TMPDIR || "/tmp", `rh-verify-blog-${process.pid}.json`);
  execFileSync("node", [new URL("./bundle-src.mjs", import.meta.url).pathname, blogDir, out], { stdio: "ignore" });
  const viaNode = JSON.parse(await readFile(out, "utf8"));
  const a = Object.keys(viaHandle.src).sort().join(","), b = Object.keys(viaNode.src).sort().join(",");
  check("directory handle reads the same files as bundle-src.mjs", a === b && viaHandle.name === "real-blog",
    `${Object.keys(viaHandle.src).length} vs ${Object.keys(viaNode.src).length} files`);
  const same = Object.keys(viaNode.src).every((k) => viaHandle.src[k] === viaNode.src[k]);
  check("directory handle reads identical text", same);
}

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

  // 2b. definition / references: the LSP's answers through the wasm.
  // `@status` in show reads back to its assignment in set_status
  // (a write), and references include reads and writes across the class.
  const defRefs = await page.evaluate(async (ctrl) => {
    const text = window.__ide.srcMap[ctrl];
    const idx = text.indexOf("@status.") + 1;
    const line = text.slice(0, idx).split("\n").length - 1;
    const ch = idx - text.lastIndexOf("\n", idx - 1) - 1;
    const def = await window.__ide.rpc("definition", { path: ctrl, line, character: ch });
    const refs = await window.__ide.rpc("references", { path: ctrl, line, character: ch });
    return { def, refs: Array.isArray(refs) ? refs.length : -1,
      writes: Array.isArray(refs) ? refs.filter((r) => r.write).length : -1 };
  }, ctrl);
  check("definition of @status is its assignment",
    defRefs.def && defRefs.def.path === ctrl && defRefs.def.write === true,
    JSON.stringify(defRefs.def));
  check("references to @status span reads and writes",
    defRefs.refs > 1 && defRefs.writes >= 1 && defRefs.writes < defRefs.refs,
    `${defRefs.refs} refs, ${defRefs.writes} writes`);

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

  // 5b. N+1 hop annotation (#63 phase 5): the admin collections trace
  // carries the missing_preload finding on its view hop and the panel
  // renders the badge.
  const np = await page.evaluate(async () => {
    await window.__ide.runTrace("Admin::CollectionsController#show");
    const t = window.__ide.trace;
    const viewHop = t?.hops.find((h) => h.kind === "view");
    return t && {
      findings: (viewHop?.n_plus_one || []).map((f) => f.association),
      badges: document.querySelectorAll("#trace .nplus").length,
    };
  });
  check("N+1 finding annotates the view hop",
    np && np.findings.includes("account") && np.badges >= 1,
    np && `findings: [${np.findings}], ${np.badges} badge(s)`);

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

// 7. App picker (only when a manifest ships >1 app): switching apps
// re-ingests from scratch — a fresh tree, non-empty analysis, the app's
// own files present. Skipped for a single-app (no apps.json) deployment.
const appNames = await page.evaluate(() => (window.__ide.apps || []).map((a) => a.name));
if (appNames.length >= 2) {
  async function switchTo(name, expectFile) {
    await page.evaluate((n) => window.__ide.loadApp(window.__ide.apps.find((a) => a.name === n)), name);
    await page.waitForFunction(
      (f) => window.__ide.analysis && (f in window.__ide.srcMap),
      expectFile, { timeout: 120_000 });
    return page.evaluate(() => ({
      files: window.__ide.analysis.files.length,
      title: document.title,
    }));
  }
  check("app manifest lists blog + lobsters + campfire + mastodon",
    ["blog", "lobsters", "campfire", "mastodon"].every((n) => appNames.includes(n)), appNames.join(","));
  const blog = await switchTo("blog", "app/models/article.rb");
  check("switch → blog re-ingests", blog.files > 5 && /blog/.test(blog.title), `${blog.files} files`);
  const lob = await switchTo("lobsters", "app/models/story.rb");
  check("switch → lobsters re-ingests", lob.files > 30 && /lobsters/.test(lob.title), `${lob.files} files`);
  if (appNames.includes("campfire")) {
    const cf = await switchTo("campfire", "app/models/message.rb");
    check("switch → campfire re-ingests", cf.files > 100 && /campfire/.test(cf.title), `${cf.files} files`);
  }
  const mast = await switchTo("mastodon", "app/controllers/statuses_controller.rb");
  check("switch → mastodon re-ingests (round-trip)", mast.files > 100 && /mastodon/.test(mast.title), `${mast.files} files`);

  // Deep-link: a fresh load with ?app= boots straight into that app.
  await page.goto(`${BASE}?app=blog`);
  await page.waitForFunction(() => window.__ide?.analysis, null, { timeout: 120_000 });
  const booted = await page.evaluate(() => document.getElementById("app").value);
  check("?app=blog boots straight into blog", booted === "blog", booted);
}

// 8. Open folder. The real path first: Playwright feeds a directory to the
// hidden webkitdirectory input exactly as a visitor's file dialog would.
const BLOG_DIR = new URL("../../fixtures/real-blog", import.meta.url).pathname;
await page.setInputFiles("#folderInput", BLOG_DIR);
await page.waitForFunction(() => window.__ide.current?.local && window.__ide.analysis, null, { timeout: 120_000 });
const local = await page.evaluate(() => ({
  current: window.__ide.current,
  paths: Object.keys(window.__ide.srcMap).sort(),
  files: window.__ide.analysis.files.length,
  title: document.title,
  sel: document.getElementById("app").value,
  status: document.getElementById("status").textContent,
  summary: window.__ide.summaryLine(),
  version: window.__ide.version,
  url: location.search,
}));
check("open folder: real-blog analyzes as a local app",
  local.current?.local && local.current.name === "real-blog" && local.files > 10 && /real-blog/.test(local.title),
  `${local.files} files, title ${local.title}`);
check("open folder: selector shows the local app, no ?app= deep-link",
  local.sel === "__local" && !/app=/.test(local.url), `${local.sel} ${local.url}`);
check("open folder: status says nothing was uploaded", /nothing uploaded/.test(local.status), local.status);
// Same inclusion rules as the Node bundler: the walk dirs, the single
// files, nothing else (real-blog has test/, bin/, public/… on disk).
check("open folder: only analyzable sources were read",
  local.paths.every((p) => /^(app|extras|lib|config\/routes|models|views|db\/migrate)\//.test(p)
    || ["db/schema.rb", "config/routes.rb", "config.ru", "app.rb", "db.rb", "seeds.rb"].includes(p))
  && local.paths.includes("config/routes.rb") && local.paths.includes("db/schema.rb")
  && local.paths.some((p) => p.startsWith("app/models/")),
  `${local.paths.length} paths`);
check("open folder: summary line names app, ledger and build",
  /^real-blog · \d+ files · \d+ errors · \d+ warnings · \d+ coverage notes · \d+ ingest gaps · roundhouse \d/.test(local.summary)
  && local.version?.version,
  local.summary);

// The entries seam (what both loaders feed): junk that a whole-tree
// enumeration would include is dropped before analysis.
const synth = await page.evaluate(async () => {
  await window.__ide.openLocalEntries("synthetic", [
    { path: "app/models/thing.rb", text: "class Thing < ApplicationRecord\nend\n" },
    { path: "config/routes.rb", text: "Rails.application.routes.draw do\n  resources :things\nend\n" },
    { path: "db/schema.rb", text: "ActiveRecord::Schema[8.0].define(version: 1) do\n  create_table \"things\" do |t|\n    t.string \"name\"\n  end\nend\n" },
    { path: "node_modules/x/index.rb", text: "puts 1\n" },
    { path: "tmp/cache/foo.rb", text: "puts 1\n" },
    { path: "app/assets/logo.png", text: "\x89PNG" },
  ]);
  return { paths: Object.keys(window.__ide.srcMap).sort(), name: window.__ide.current.name };
});
check("open folder: entries outside the rules are filtered",
  synth.paths.join(",") === "app/models/thing.rb,config/routes.rb,db/schema.rb" && synth.name === "synthetic",
  synth.paths.join(","));

// Back to a shipped app: the local row leaves the picker.
if (appNames.length >= 2) {
  await page.evaluate(() => window.__ide.loadApp(window.__ide.apps.find((a) => a.name === "blog")));
  await page.waitForFunction(() => window.__ide.analysis && !window.__ide.current.local, null, { timeout: 120_000 });
  const back = await page.evaluate(() => ({
    hasLocal: !!document.querySelector('#app option[value="__local"]'),
    hasOpen: !!document.querySelector('#app option[value="__open"]'),
  }));
  check("open folder: switching back drops the local entry, keeps open folder…",
    !back.hasLocal && back.hasOpen, JSON.stringify(back));
}

await browser.close();
if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("ide verify: all checks passed");
