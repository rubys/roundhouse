// roundhouse studio — rung D of docs/browser-demo-plan.md.
//
// "The blog, editable": edit Ruby → recompile in-browser → run the emitted
// TypeScript blog live. The whole loop is client-side (decisions #1/#5/#6):
//
//   edit → wasm transpile (worker profile) → esbuild bundle → host the bundles
//   in a service worker → (re)load the app in an iframe over sqlite-wasm.
//
// Phase 5 ships the FULL-RELOAD loop: each edit re-bundles and reloads the
// iframe; the app's OPFS DB persists across reloads, so state survives a
// recompile. (True module hot-swap is a later polish.)
//
// Serve the PARENT (wasm/) as the web root so /studio/ and /lib/ both resolve.

import { loadDefaultCompiler, loadFixture } from "../lib/transpile.mjs";
import { createEditor } from "../lib/editor.js";
import { allDirPaths, renderTree } from "../lib/tree.js";
import { loadBundler } from "../lib/bundle.mjs";
import { createAppHost } from "../lib/app-host.mjs";
import { originalPositionFor, normPath } from "../lib/sourcemap.mjs";

const TARGET = "typescript"; // studio is TS-only — the only browser runtime
const PROFILE = "worker";    // the SharedWorker browser app (what studio runs)
const DEBOUNCE_MS = 300;
const DEFAULT_FILE = "app/views/articles/index.html.erb"; // a view: most visibly "live"

// The app's HTML shell (the worker target's index.html, adapted: bundle
// basenames + app-relative worker URLs + Tailwind's browser JIT build for live
// styling). Served by the SW at the app scope root.
//
// The bundle URLs carry a per-build `?v=` so a recompile actually takes effect:
// SharedWorkers are keyed by URL, so reusing a fixed `worker.js` would reload
// the iframe but keep the OLD worker (where view/controller rendering happens) —
// the edit wouldn't show. A fresh `?v=` mints a fresh worker each build. The SW
// matches on pathname (ignoring the query), so all versions resolve to the same
// served file. The DB worker also respawns, but reopens the same OPFS pool, so
// data persists across edits.
function appShell(v) {
  return `<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Roundhouse App</title>
<meta name="turbo-refresh-method" content="morph">
<meta name="turbo-refresh-scroll" content="preserve">
<link rel="icon" href="data:,">
<meta name="juntos-worker" content="worker.js?v=${v}">
<meta name="juntos-db-worker" content="db_worker.js?v=${v}">
<script src="https://cdn.jsdelivr.net/npm/@tailwindcss/browser@4"></script>
</head><body>
<div id="loading" style="padding:16px;color:#666;font:14px system-ui">Loading…</div>
<div id="app" style="display:none"></div>
<script type="module" src="main.js?v=${v}"></script>
</body></html>`;
}

const els = {
  status: document.getElementById("status"),
  srcfiles: document.getElementById("srcfiles"),
  editorHost: document.getElementById("editorHost"),
  editorHead: document.getElementById("editorHead"),
  appStatus: document.getElementById("appStatus"),
  appFrame: document.getElementById("appFrame"),
  rightTabs: document.getElementById("rightTabs"),
  appPane: document.getElementById("appPane"),
  testsPane: document.getElementById("testsPane"),
  tabBadge: document.getElementById("tabBadge"),
  runTestsBtn: document.getElementById("runTestsBtn"),
  testSummary: document.getElementById("testSummary"),
  conformance: document.getElementById("conformance"),
  testResults: document.getElementById("testResults"),
};

// The app runs at <studio>/app/ (a SW-served subtree); sw.js sits beside this
// page. Both are computed from location so they work locally and under the
// /roundhouse/ Pages mount alike.
const appScope = new URL("app/", location.href).pathname;
const swUrl = new URL("sw.js", location.href).href;

let compiler = null;
let bundler = null;         // esbuild-wasm bundler (null if its CDN load failed)
let appHost = null;         // SW-backed iframe host (null if SW unavailable)
let editor = null;
let srcMap = null;          // { path: content } — the live, editable input (Ruby)
let currentPath = null;
let openDirs = null;
let lastBuild = null;       // { files, diagnostics, types, error, transpileMs, testSuite }
let lastBundle = null;      // { ms, errors, warnings, outputs }
let lastTestRun = null;     // { total, passed, failed, skipped, results } | { error }
let buildSeq = 0;           // guards stale async builds
let debounceTimer = null;
let activeTab = "app";      // "app" | "tests" (right-column tab)
let testsStale = true;      // emitted suite changed since the last run
let testRunPromise = null;  // the in-flight run+paint (coalesces concurrent calls)
let lastSchema = null;      // emitted src/schema.ts — a change forces a full reload

function setStatus(msg, kind = "") { els.status.textContent = msg; els.status.className = kind; }
function setAppStatus(html) { els.appStatus.innerHTML = html; }

// ---- source tree ---------------------------------------------------------

function sourceFiles() {
  return Object.keys(srcMap).filter((p) => /\.(rb|erb|ru)$/.test(p)).sort();
}

function renderSources() {
  const paths = sourceFiles();
  if (openDirs === null) {
    openDirs = new Set([...allDirPaths(paths)].filter((d) => d === "app" || d.startsWith("app/")));
  }
  renderTree(els.srcfiles, paths, {
    isOpen: (d) => openDirs.has(d),
    toggleDir: (d) => { openDirs.has(d) ? openDirs.delete(d) : openDirs.add(d); },
    isActive: (p) => p === currentPath,
    onPick: selectFile,
  });
}

function langForPath(path) { return path.endsWith(".erb") ? "html" : "ruby"; }

function selectFile(path) {
  currentPath = path;
  editor.setValue(srcMap[path], langForPath(path));
  els.editorHead.textContent = path;
  [...els.srcfiles.querySelectorAll("button")].forEach((b) =>
    b.classList.toggle("active", b.dataset.path === path));
  renderMarkers();
  renderTypes(); // hovers are per-file — refresh for the newly-open file
}

// ---- the emitted test suite (Phase 6) ------------------------------------

// roundhouse already transpiles the Rails-style Minitest suites to TS under
// the worker profile — the spec files (`test/<x>.test.ts`), the in-browser
// harness (`test/_runtime/minitest.ts` + `setup.ts`), and the fixtures
// (`test/fixtures/*.ts`). They ride in the build output but the running-app
// loop ignores them; this picks them out so the suite is a first-class part
// of the payload (Phase 7's runner consumes it; the verifier proves it
// reaches the browser). The matching test SOURCES (`test/**/*_test.rb`) are
// already in `srcMap`, so they show in the source tree and are editable.
function testSuiteFrom(files) {
  const pick = (re) => files.filter((f) => re.test(f.path) && !f.path.endsWith(".map"));
  return {
    specs: pick(/^test\/.*\.test\.ts$/),       // the per-class *Test suites
    runtime: pick(/^test\/_runtime\/.*\.ts$/), // minitest harness + setup
    fixtures: pick(/^test\/fixtures\/.*\.ts$/),// FixtureLoader inputs
  };
}

// ---- the loop: transpile → bundle → host → (re)load ----------------------

function onEditorChange(value) {
  if (currentPath == null) return;
  srcMap[currentPath] = value;
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(build, DEBOUNCE_MS);
}

async function build() {
  const seq = ++buildSeq;

  // 1. Ruby → worker-profile TypeScript (in-browser wasm).
  const t0 = performance.now();
  let out;
  try {
    out = compiler.transpile(TARGET, srcMap, { profile: PROFILE });
  } catch (e) {
    out = { error: `transpile threw: ${e.message}` };
  }
  lastBuild = {
    files: out.files || [],
    diagnostics: out.diagnostics || [],
    types: out.inferred_types || [], // inferred-type spans → editor hover tooltips
    error: out.error || null,
    transpileMs: performance.now() - t0,
  };
  lastBuild.testSuite = testSuiteFrom(lastBuild.files); // Phase 6: ship the suite
  renderMarkers();
  renderTypes();
  if (out.error) {
    setStatus(`transpile error: ${out.error}`, "err");
    return;
  }
  const errs = lastBuild.diagnostics.filter((d) => d.severity === "error").length;
  const suites = lastBuild.testSuite.specs.length;
  // Errors take the slot when present (the suite may be incomplete); otherwise
  // report how many test suites shipped in the payload. Phase 6 only *ships*
  // the suite — running it (green/red) is Phase 7-8, so don't imply a result.
  const detail = errs
    ? ` · ${errs} error${errs > 1 ? "s" : ""}`
    : suites ? ` · ${suites} test suite${suites > 1 ? "s" : ""}` : "";
  setStatus(`${lastBuild.files.length} TS files${detail} in ${lastBuild.transpileMs.toFixed(0)} ms`, errs ? "err" : "ok");

  // The emitted suite just changed; mark the last run stale. Re-run eagerly
  // only while the tests tab is open (so editing a test/model shows live
  // green/red); otherwise the badge waits until you switch to the tab.
  testsStale = true;
  if (activeTab === "tests" && !testRunPromise) runAndRenderTests();
  if (!bundler) { setAppStatus(`<span class="err">esbuild unavailable</span> — transpile-only`); return; }

  // 2. Bundle the emitted TS → 3 browser-loadable ESM bundles.
  setAppStatus(`<span class="k">bundling…</span>`);
  const emitted = Object.fromEntries(lastBuild.files.map((f) => [f.path, f.content]));
  let b;
  try {
    b = await bundler.bundle(emitted, undefined, { base: appScope });
  } catch (e) {
    b = { ms: 0, errors: [{ text: `bundler threw: ${e.message}` }], warnings: [], outputs: {} };
  }
  if (seq !== buildSeq) return; // superseded
  lastBundle = b;
  if (b.errors.length) {
    setAppStatus(`<span class="err">bundle: ${escapeHtml(b.errors[0].text)}</span>`);
    return;
  }
  const sizes = Object.entries(b.outputs).map(([n, o]) => `${n} ${(o.bytes / 1024).toFixed(0)}K`).join(" ");
  if (!appHost) { setAppStatus(`<span class="k">bundle</span> ${sizes} · ${b.ms.toFixed(0)}ms — app host unavailable`); return; }

  // 3. Host the bundles in the SW, then update the running app. Prefer a
  // HOT-SWAP (respawn just the SharedWorker + Turbo-morph in place, keeping the
  // iframe/main-thread/DB alive → scroll+focus preserved); fall back to a full
  // iframe reload on first mount, on a schema change (the in-memory DB layout
  // can't morph), or if the app doesn't ack the swap.
  setAppStatus(`<span class="k">bundle</span> ${sizes} · ${b.ms.toFixed(0)}ms · <span class="k">loading app…</span>`);
  const files = {
    "index.html": { body: appShell(seq), type: "text/html; charset=utf-8" },
    "main.js": { body: b.outputs["main.js"].text, type: "text/javascript" },
    "worker.js": { body: b.outputs["worker.js"].text, type: "text/javascript" },
    "db_worker.js": { body: b.outputs["db_worker.js"].text, type: "text/javascript" },
  };
  const schema = lastBuild.files.find((f) => f.path === "src/schema.ts")?.content ?? "";
  const schemaChanged = lastSchema !== null && schema !== lastSchema;
  lastSchema = schema;
  try {
    let swapped = false;
    if (!schemaChanged && appHost.hotSwap) swapped = await appHost.hotSwap(files, seq);
    if (!swapped) await appHost.update(files);
    setAppStatus(`<span class="k">bundle</span> ${sizes} · <span class="ok">${swapped ? "hot-swapped" : "running"}</span>`);
  } catch (e) {
    setAppStatus(`<span class="err">app load failed: ${escapeHtml(e.message)}</span>`);
  }
}

// ---- the test runner (Phase 7) -------------------------------------------

// Run ONE bundled ESM module in a throwaway module Worker; resolve with the
// {spec,total,passed,failed,skipped,results} summary it posts back. Each worker
// gets a FRESH in-memory sqlite-wasm DB — never the live app's opfs pool
// (risk #4).
function runOneInWorker(text) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/javascript" }));
  return new Promise((resolve, reject) => {
    const w = new Worker(url, { type: "module" });
    const done = (fn, arg) => { clearTimeout(timer); w.terminate(); URL.revokeObjectURL(url); fn(arg); };
    const timer = setTimeout(() => done(reject, new Error("test run timed out")), 30000);
    w.onmessage = (e) => { if (e.data?.type === "rh-test-results") done(resolve, e.data.summary); };
    w.onerror = (e) => done(reject, new Error(e.message || "test worker error"));
  });
}

// Bundle the emitted Minitest suite (in-memory DB + node:test/assert shims;
// see ../lib/test-runtime.mjs) and run it. Each spec FILE runs in its own
// worker with its own fresh in-memory DB — the per-file isolation `node --test`
// gives in CI, so a spec that mutates fixtures can't leak into the next file.
// Returns an aggregate { total, passed, failed, skipped, results, files } or
// { error }.
async function runTests() {
  if (!bundler) return { error: "esbuild unavailable" };
  if (!lastBuild || lastBuild.error) return { error: "no successful build to test" };
  const emitted = Object.fromEntries(lastBuild.files.map((f) => [f.path, f.content]));
  let tb;
  try {
    tb = await bundler.bundleTests(emitted, { base: appScope });
  } catch (e) {
    return { error: `test bundle threw: ${e.message}` };
  }
  if (tb.errors.length || tb.outputs.length === 0) {
    return { error: `test bundle: ${tb.errors[0]?.text || "no output"}` };
  }
  try {
    const files = [];
    // Sequential: one in-memory sqlite-wasm per worker; no need to run 4 at once.
    for (const o of tb.outputs) {
      if (!o.text) { files.push({ spec: o.spec, error: "no bundle output" }); continue; }
      files.push(await runOneInWorker(o.text));
    }
    const ran = files.filter((f) => f.results);
    const agg = (k) => ran.reduce((n, f) => n + f[k], 0);
    lastTestRun = {
      total: agg("total"), passed: agg("passed"), failed: agg("failed"), skipped: agg("skipped"),
      // Tag each result with its spec file so the panel can map it back to Ruby.
      results: ran.flatMap((f) => f.results.map((r) => ({ ...r, spec: f.spec }))),
      files,
      bundleMs: tb.ms,
      bundleBytes: tb.outputs.reduce((n, o) => n + (o.bytes || 0), 0),
    };
    return lastTestRun;
  } catch (e) {
    lastTestRun = { error: e.message };
    return lastTestRun;
  }
}

// ---- the test panel UI (Phase 8) -----------------------------------------

// The other 8 targets the same Minitest suite is emitted to. Only TypeScript
// runs LIVE in the browser (it's the one with a browser runtime); the rest are
// CI-attested — the same suite passes when emitted to them and run in CI
// (decision #5). Labelled as such; never faked as a live result.
const CONFORMANCE = [
  ["go", "Go"], ["rust", "Rust"], ["python", "Python"], ["crystal", "Crystal"],
  ["elixir", "Elixir"], ["kotlin", "Kotlin"], ["swift", "Swift"], ["ruby", "Ruby"],
];

function setTab(tab) {
  activeTab = tab;
  [...els.rightTabs.querySelectorAll(".tab")].forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
  els.appPane.hidden = tab !== "app";
  els.testsPane.hidden = tab !== "tests";
  if (tab === "tests" && testsStale && !testRunPromise) runAndRenderTests();
}

function prettyMethod(m) {
  return m.replace(/^is_/, "").replace(/^test_/, "").replace(/_/g, " ");
}

// Phase 9: map a test result back to its Ruby source location — the "debug" leg
// (a failing test points to the line of Ruby the user wrote, not the emitted
// TS). The emitted token-level `.test.ts.map` names the Ruby spec in its
// `sources`; from there the `test "..."` declaration line is found by a
// punctuation-tolerant search keyed on the method name (the mangling
// test "should get index" → test_should_get_index is lossy, so `_` matches any
// non-alphanumeric run). Falls back to the raw sourcemap position if the
// declaration can't be located. Returns { path, line } in the Ruby source, or
// null.
function rubyLocForResult(r) {
  if (!r || !r.spec || !lastBuild) return null;
  const ts = lastBuild.files.find((f) => f.path === r.spec);
  const mapF = lastBuild.files.find((f) => f.path === r.spec + ".map");
  if (!ts || !mapF) return null;
  let map;
  try { map = JSON.parse(mapF.content); } catch { return null; }
  if (!map.sources || !map.sources.length) return null;
  const rubyPath = normPath(r.spec.replace(/[^/]*$/, "") + map.sources[0]); // .map dir + source
  if (srcMap[rubyPath] == null) return null;

  const method = r.name.includes("#") ? r.name.slice(r.name.indexOf("#") + 1) : r.name;
  const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

  // 1) the `test "..."` declaration line for this method.
  const words = method.replace(/^test_/, "").split(/_+/).filter(Boolean).map(esc);
  if (words.length) {
    const re = new RegExp(`\\btest\\s+["'].*?${words.join("[^A-Za-z0-9]+")}.*?["']`);
    const rb = srcMap[rubyPath].split("\n");
    for (let i = 0; i < rb.length; i++) if (re.test(rb[i])) return { path: rubyPath, line: i + 1 };
  }

  // 2) fall back to the emitted sourcemap: the method's first mapped line.
  const tsLines = ts.content.split("\n");
  const decl = new RegExp("\\b" + esc(method) + "\\s*\\(");
  const declLine0 = tsLines.findIndex((l) => decl.test(l));
  if (declLine0 >= 0) {
    for (let g = declLine0; g <= Math.min(declLine0 + 12, tsLines.length - 1); g++) {
      const pos = originalPositionFor(map, g + 1, 0);
      if (pos) return { path: rubyPath, line: pos.line };
    }
  }
  return { path: rubyPath, line: 1 };
}

// Open a Ruby source location in the editor: reveal its dir in the tree, select
// the file, scroll + flash the line.
function jumpToSource(loc) {
  if (!loc || srcMap[loc.path] == null) return false;
  const dirs = loc.path.split("/").slice(0, -1);
  let d = "";
  for (const seg of dirs) { d = d ? d + "/" + seg : seg; if (openDirs) openDirs.add(d); }
  renderSources();
  selectFile(loc.path);
  editor.revealLine?.(loc.line);
  return true;
}

function renderTabBadge(run) {
  const b = els.tabBadge;
  if (!run) { b.hidden = true; return; }
  b.hidden = false;
  if (run.running) { b.className = "tbadge run"; b.textContent = "…"; return; }
  if (run.error) { b.className = "tbadge err"; b.textContent = "err"; return; }
  b.className = "tbadge " + (run.failed ? "err" : "ok");
  b.textContent = `${run.passed}/${run.total}`;
}

function renderTestSummary(run) {
  if (run.running) { els.testSummary.className = "k"; els.testSummary.textContent = "running…"; return; }
  if (run.error) { els.testSummary.className = "err"; els.testSummary.textContent = run.error; return; }
  const cls = run.failed ? "err" : "ok";
  els.testSummary.innerHTML = `<span class="${cls}">${run.passed} passed`
    + `${run.failed ? `, ${run.failed} failed` : ""}${run.skipped ? `, ${run.skipped} skipped` : ""}</span>`
    + ` <span class="k">· ${run.total} tests · ${run.bundleMs != null ? run.bundleMs.toFixed(0) : "?"}ms bundle</span>`;
}

function renderConformance(run) {
  const tsErr = run && !run.error && run.failed > 0;
  const tsTxt = run && !run.error ? `TS ${run.passed}/${run.total}` : (run?.running ? "TS …" : "TS —");
  const chips = [`<span class="chip live${tsErr ? " err" : ""}">${tsTxt} <span class="m">live</span></span>`]
    .concat(CONFORMANCE.map(([k, name]) =>
      `<span class="chip" title="the same Minitest suite passes when emitted to ${name} and run in CI">${name} ✓ <span class="m">CI</span></span>`));
  els.conformance.innerHTML =
    `<div>same Minitest suite, 9 targets — TypeScript runs <b>live, here in your browser</b>; the rest are <b>CI-attested</b>.</div>`
    + `<div class="row">${chips.join("")}</div>`;
}

function renderTestResults(run) {
  if (run.error) { els.testResults.innerHTML = `<div class="empty">test run failed: ${escapeHtml(run.error)}</div>`; return; }
  // Group flat `ClassName#test_method` results by suite class (≈ one per file),
  // keeping each result's index into run.results for click→source.
  const groups = new Map();
  run.results.forEach((r, ri) => {
    const hash = r.name.indexOf("#");
    const suite = hash >= 0 ? r.name.slice(0, hash) : r.name;
    const method = hash >= 0 ? r.name.slice(hash + 1) : r.name;
    if (!groups.has(suite)) groups.set(suite, []);
    groups.get(suite).push({ ...r, method, ri });
  });
  let html = "";
  for (const [suite, cases] of groups) {
    const fails = cases.filter((c) => c.status === "fail").length;
    html += `<div class="suite"><div class="suite-h">${escapeHtml(suite)} <span class="m">${cases.length - fails}/${cases.length}</span></div>`;
    for (const c of cases) {
      const ico = c.status === "pass" ? "✓" : c.status === "skip" ? "‒" : "✗";
      html += `<div class="tcase ${c.status} clickable" data-ri="${c.ri}" title="open this test in the editor">`
        + `<span class="ico">${ico}</span>`
        + `<span class="nm">${escapeHtml(prettyMethod(c.method))}</span>`
        + `<span class="ms">${c.ms != null ? c.ms.toFixed(0) + "ms" : ""}</span></div>`;
      if (c.status === "fail" && c.error) html += `<div class="terr">${escapeHtml(c.error)}</div>`;
    }
    html += `</div>`;
  }
  els.testResults.innerHTML = html || `<div class="empty">no tests</div>`;
}

// Click a result row → jump to its Ruby source (Phase 9).
function onTestResultsClick(e) {
  const row = e.target.closest(".tcase");
  if (!row || !lastTestRun?.results) return;
  const r = lastTestRun.results[Number(row.dataset.ri)];
  const loc = rubyLocForResult(r);
  if (loc) jumpToSource(loc);
}

// Run the suite and paint the panel (summary + tab badge + conformance + tree).
// Concurrent callers (e.g. a tab-switch auto-run racing an explicit Run) share
// the one in-flight promise — including the paint — so nobody gets stale data.
function runAndRenderTests() {
  if (testRunPromise) return testRunPromise;
  testRunPromise = (async () => {
    testsStale = false;
    els.runTestsBtn.disabled = true;
    renderTabBadge({ running: true });
    renderTestSummary({ running: true });
    renderConformance({ running: true });
    try {
      const run = await runTests();
      renderTabBadge(run);
      renderTestSummary(run);
      renderConformance(run);
      renderTestResults(run);
      return run;
    } finally {
      els.runTestsBtn.disabled = false;
      testRunPromise = null;
    }
  })();
  return testRunPromise;
}

function renderMarkers() {
  if (!editor || !lastBuild) return;
  editor.setMarkers(lastBuild.diagnostics.filter((d) => d.path === currentPath));
}

// Inferred types for the open file → editor hover tooltips (same source the
// playground feeds Monaco). Per-file, so refresh on each build and file switch.
function renderTypes() {
  if (!editor || !lastBuild) return;
  editor.setTypes((lastBuild.types || []).filter((t) => t.path === currentPath));
}

function escapeHtml(s) {
  return String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
}

// ---- boot ----------------------------------------------------------------

async function boot() {
  setStatus("loading wasm + fixture…");
  const [loaded, fixture] = await Promise.all([
    loadDefaultCompiler({ onStdout: (s) => console.log("[wasm]", s), onStderr: (s) => console.warn("[wasm]", s) }),
    loadFixture(),
  ]);
  compiler = loaded;
  srcMap = fixture;

  renderSources();
  setStatus("loading editor + bundler…");
  // Editor, esbuild, and the SW app-host load in parallel; each degrades
  // independently (offline / strict CSP / no SW) without failing the boot.
  const [ed, bnd, host] = await Promise.all([
    createEditor(els.editorHost, {
      onChange: onEditorChange,
      // Typed completion from the last build's analysis snapshot (the
      // wasm side stashes one on every transpile — see lib/transpile.mjs).
      complete: (text, line, character) =>
        currentPath ? compiler.complete(currentPath, text, line, character) : [],
    }),
    loadBundler().catch((e) => { console.warn("[studio] bundler unavailable:", e.message); return null; }),
    createAppHost(els.appFrame, { swUrl, scope: appScope }).catch((e) => { console.warn("[studio] app host unavailable:", e.message); return null; }),
  ]);
  editor = ed;
  bundler = bnd;
  appHost = host;
  if (!host) setAppStatus(`<span class="err">app host unavailable</span> (no Service Worker)`);

  // Right-column tabs + test-panel controls.
  els.rightTabs.addEventListener("click", (e) => {
    const tab = e.target.closest(".tab")?.dataset.tab;
    if (tab) setTab(tab);
  });
  els.runTestsBtn.addEventListener("click", () => runAndRenderTests());
  els.testResults.addEventListener("click", onTestResultsClick); // Phase 9: row → Ruby
  renderConformance(null); // static strip visible before the first run

  const first = srcMap[DEFAULT_FILE] != null ? DEFAULT_FILE : sourceFiles()[0];
  selectFile(first);
  await build();

  // Programmatic hooks for the Playwright verifier.
  window.__studio = {
    ready: true,
    editorKind: editor.kind,
    hasBundler: () => bundler != null,
    hasAppHost: () => appHost != null,
    appScope,
    selectSource: (path) => selectFile(path),
    async editFile(path, content) {
      srcMap[path] = content;
      if (path === currentPath) editor.setValue(content, langForPath(path));
      await build();
    },
    build: () => lastBuild,
    bundle: () => lastBundle,
    testSuite: () => lastBuild?.testSuite || null, // Phase 6: emitted Minitest suite
    runTests,                                       // Phase 7: run the suite in-browser
    runTestsUI: runAndRenderTests,                  // Phase 8: run + paint the panel
    testRun: () => lastTestRun,
    source: (path) => srcMap[path],
    sourceCount: () => sourceFiles().length,
    // Inferred-type hover data — parity with __playground. These read the spans
    // the studio now feeds the editor via setTypes(); the verifier asserts the
    // wiring (so a regression that drops the setTypes calls is caught).
    types: () => lastBuild?.types || [],
    // Smallest-span inferred type at a 1-based (line, col) in the open file.
    typeAt(line, col) {
      const hits = (lastBuild?.types || []).filter((t) => t.path === currentPath &&
        (line > t.start_line || (line === t.start_line && col >= t.start_col)) &&
        (line < t.end_line || (line === t.end_line && col <= t.end_col)));
      hits.sort((a, b) =>
        ((a.end_line - a.start_line) * 1e5 + (a.end_col - a.start_col)) -
        ((b.end_line - b.start_line) * 1e5 + (b.end_col - b.start_col)));
      return hits.length ? hits[0].ty : null;
    },
    selectTab: setTab,
    currentFile: () => currentPath,                                  // Phase 9
    sourceLocForTest: (name) => rubyLocForResult((lastTestRun?.results || []).find((r) => r.name === name)),
  };

  // Run the emitted suite once in the background and paint the panel/badge, so
  // the "tests N/N" badge is live even before you open the tab. Doesn't block
  // the visible app boot.
  runAndRenderTests();
}

boot().catch((e) => setStatus(`boot failed: ${e.message}`, "err"));
