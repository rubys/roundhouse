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
let lastBuild = null;       // { files, diagnostics, error, transpileMs }
let lastBundle = null;      // { ms, errors, warnings, outputs }
let buildSeq = 0;           // guards stale async builds
let debounceTimer = null;

function setStatus(msg, kind = "") { els.status.textContent = msg; els.status.className = kind; }
function setAppStatus(html) { els.appStatus.innerHTML = html; }

// ---- source tree ---------------------------------------------------------

function sourceFiles() {
  return Object.keys(srcMap).filter((p) => /\.(rb|erb)$/.test(p)).sort();
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
    error: out.error || null,
    transpileMs: performance.now() - t0,
  };
  renderMarkers();
  if (out.error) {
    setStatus(`transpile error: ${out.error}`, "err");
    return;
  }
  const errs = lastBuild.diagnostics.filter((d) => d.severity === "error").length;
  setStatus(`${lastBuild.files.length} TS files${errs ? ` · ${errs} error${errs > 1 ? "s" : ""}` : ""} in ${lastBuild.transpileMs.toFixed(0)} ms`, errs ? "err" : "ok");
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

  // 3. Host the bundles in the SW + (re)load the app iframe.
  setAppStatus(`<span class="k">bundle</span> ${sizes} · ${b.ms.toFixed(0)}ms · <span class="k">loading app…</span>`);
  const files = {
    "index.html": { body: appShell(seq), type: "text/html; charset=utf-8" },
    "main.js": { body: b.outputs["main.js"].text, type: "text/javascript" },
    "worker.js": { body: b.outputs["worker.js"].text, type: "text/javascript" },
    "db_worker.js": { body: b.outputs["db_worker.js"].text, type: "text/javascript" },
  };
  try {
    await appHost.update(files);
    setAppStatus(`<span class="k">bundle</span> ${sizes} · <span class="ok">running</span>`);
  } catch (e) {
    setAppStatus(`<span class="err">app load failed: ${escapeHtml(e.message)}</span>`);
  }
}

function renderMarkers() {
  if (!editor || !lastBuild) return;
  editor.setMarkers(lastBuild.diagnostics.filter((d) => d.path === currentPath));
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
    createEditor(els.editorHost, { onChange: onEditorChange }),
    loadBundler().catch((e) => { console.warn("[studio] bundler unavailable:", e.message); return null; }),
    createAppHost(els.appFrame, { swUrl, scope: appScope }).catch((e) => { console.warn("[studio] app host unavailable:", e.message); return null; }),
  ]);
  editor = ed;
  bundler = bnd;
  appHost = host;
  if (!host) setAppStatus(`<span class="err">app host unavailable</span> (no Service Worker)`);

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
    source: (path) => srcMap[path],
    sourceCount: () => sourceFiles().length,
  };
}

boot().catch((e) => setStatus(`boot failed: ${e.message}`, "err"));
