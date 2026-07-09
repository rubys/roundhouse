// Phase 1 — multi-target playground (rung A of docs/browser-demo-plan.md).
//
// Shared-lib model (rung D Phase 4): the compiler driver (transpile.mjs +
// wasi-shim.mjs + the roundhouse_wasm.wasm binary + the seed fixture.json), the
// editor abstraction (editor.js), and the file-tree widget (tree.js) all live
// in ../lib/ and are shared with /studio/. This dir holds only the playground's
// own UI: index.html + this file. What's playground-specific (not in lib): the
// debounced edit -> transpile -> render loop, the output-file picker, and the
// diagnostics/inferred-type overlay.
//
// Serve the PARENT (wasm/) as the web root so /playground/, /studio/, and /lib/
// all resolve (mirrors the published _site/ tree):
//   python3 -m http.server 8099   # run from wasm/
//   open http://localhost:8099/playground/

import { createClient } from "../lib/wasm-client.mjs";
import { createEditor, createOutputView } from "../lib/editor.js";
import { allDirPaths, renderTree } from "../lib/tree.js";

// The targets the wasm entry point routes to, in alphabetical order (= the
// dropdown order). (kotlin/swift/csharp use `emit()`; ruby uses `emit_spinel()`.)
const TARGETS = ["crystal", "csharp", "elixir", "go", "kotlin", "python", "ruby", "rust", "swift", "typescript"];

// Map a target to a Monaco language id for the (read-only) output pane.
// Elixir/Crystal have no built-in Monaco grammar -> plaintext.
const OUT_LANG = {
  typescript: "typescript", go: "go", rust: "rust", python: "python",
  kotlin: "kotlin", swift: "swift", csharp: "csharp", ruby: "ruby",
  elixir: "plaintext", crystal: "plaintext",
};

const DEBOUNCE_MS = 250;
const DEFAULT_FILE = "app/models/article.rb";
const DEFAULT_TARGET = "ruby"; // Ruby is the initial target (most on-message: Ruby -> typed Ruby + .rbs)

const els = {
  target: document.getElementById("target"),
  app: document.getElementById("app"),
  appLabel: document.getElementById("appLabel"),
  status: document.getElementById("status"),
  srcfiles: document.getElementById("srcfiles"),
  editorHost: document.getElementById("editorHost"),
  editorHead: document.getElementById("editorHead"),
  picker: document.getElementById("outpicker"),
  outfileBtn: document.getElementById("outfileBtn"),
  outfileMenu: document.getElementById("outfileMenu"),
  outputHost: document.getElementById("outputHost"),
};

let client = null;         // shared off-thread compiler client (lib/wasm-client)
let apps = [];              // app-picker manifest entries ({name,label,src})
let transpileSeq = 0;      // guards against a stale async transpile overwriting a newer one
let editor = null;
let outputView = null;      // read-only Monaco (or <pre>) showing the emitted file
let srcMap = null;          // { path: content } — the live, editable input
let currentPath = null;     // which source file the editor is showing
let currentOutIndex = 0;    // which emitted file the output pane is showing
let currentOutPath = null;  // path of the emitted file shown (to detect re-renders of the same file)
let openDirs = null;        // Set<string> of expanded directory paths in the source tree
let outClosedDirs = new Set(); // collapsed dirs in the output picker (default: all open)
let lastOutput = null;      // last { language, files } | { error }
let lastDiagnostics = [];   // last result's diagnostics (target-independent)
let lastTypes = [];         // last result's inferred types (target-independent)
let debounceTimer = null;

function setStatus(msg, kind = "") {
  els.status.textContent = msg;
  els.status.className = kind;
}

// ---- source tree ---------------------------------------------------------

function sourceFiles() {
  return Object.keys(srcMap).filter((p) => /\.(rb|erb)$/.test(p)).sort();
}

// buildTree / allDirPaths / renderTree now live in ../lib/tree.js (shared with
// studio). renderSources + renderOutTree below drive that widget with the
// playground's own open-state and pick handlers.

function renderSources() {
  const paths = sourceFiles();
  // First render: expand the app/ subtree (the interesting code), collapse the
  // rest (config/db/test). Subsequent renders preserve the user's toggles.
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

function langForPath(path) {
  return path.endsWith(".erb") ? "html" : "ruby";
}

function selectFile(path) {
  currentPath = path;
  editor.setValue(srcMap[path], langForPath(path));
  els.editorHead.textContent = path;
  [...els.srcfiles.querySelectorAll("button")].forEach((b) =>
    b.classList.toggle("active", b.dataset.path === path));
  renderMarkers(); // squiggles are per-file — refresh for the newly-open file
  renderTypes();   // hovers too
  const oi = outputIndexForSource(path); // show this source's emitted file, if found
  if (oi >= 0) showOutput(oi);
}

// ---- transpile loop ------------------------------------------------------

function onEditorChange(value) {
  if (currentPath == null) return;
  srcMap[currentPath] = value;
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(transpile, DEBOUNCE_MS);
}

// Transpile runs off the main thread (lib/wasm-client), so a big app (Mastodon
// is multi-second) leaves the editor responsive. It's async: a `seq` guard
// drops a result that a newer edit has already superseded, and a worker
// timeout/crash comes back as an { error } we render like any other.
async function transpile() {
  const lang = els.target.value;
  const seq = ++transpileSeq;
  const t0 = performance.now();
  setStatus("transpiling…");
  let result;
  try {
    result = await client.transpile(lang, srcMap);
  } catch (e) {
    result = { error: `transpile failed: ${e.message}` };
  }
  if (seq !== transpileSeq) return; // a newer transpile is already in flight
  lastOutput = result;
  lastDiagnostics = (result && result.diagnostics) || [];
  lastTypes = (result && result.inferred_types) || [];
  renderOutput(result, performance.now() - t0);
  renderMarkers();
  renderTypes();
}

// Inference diagnostics (target-independent): squiggles on the open file +
// a count in the status bar. Editing to introduce a type error (e.g.
// `title + 1`) surfaces a red marker live — the inference-first demo.
function renderMarkers() {
  if (!editor) return;
  editor.setMarkers(lastDiagnostics.filter((d) => d.path === currentPath));
}

// Inferred types for the open file → hover tooltips (Monaco only).
function renderTypes() {
  if (!editor) return;
  editor.setTypes(lastTypes.filter((t) => t.path === currentPath));
}

function diagSummary(diags) {
  if (!diags.length) return "";
  const errs = diags.filter((d) => d.severity === "error").length;
  const warns = diags.length - errs;
  const parts = [];
  if (errs) parts.push(`${errs} error${errs > 1 ? "s" : ""}`);
  if (warns) parts.push(`${warns} warning${warns > 1 ? "s" : ""}`);
  return " · " + parts.join(", ");
}

// ---- output pane ---------------------------------------------------------

function outFiles() {
  return (lastOutput && lastOutput.files) || [];
}

// Split a path into segments, stripping the basename's extension chain
// ("app/views/articles/index.html.erb" -> ["app","views","articles","index"]).
function stemSegs(path) {
  const segs = path.split("/").filter(Boolean);
  if (segs.length) {
    const last = segs[segs.length - 1];
    const dot = last.indexOf(".");
    segs[segs.length - 1] = dot > 0 ? last.slice(0, dot) : last;
  }
  return segs;
}

// True if `segs` ends with the segment sequence `suffix`.
function endsWithSegs(segs, suffix) {
  if (segs.length < suffix.length) return false;
  const off = segs.length - suffix.length;
  return suffix.every((s, i) => segs[off + i] === s);
}

// Map the open SOURCE file to its emitted file by NAME, not by authoritative
// provenance (we have no `source` field on EmittedFile). Match on basename;
// when several outputs share it, tighten by extending the matched path suffix
// one parent dir at a time until exactly one remains. If it never reduces to a
// single match, return -1 (leave the pane alone — a wrong jump is worse than
// none). This is layout-prefix agnostic, so e.g. rust's
// src/controllers/application_controller.rs still maps from
// app/controllers/application_controller.rb. `.map`/`.rbs` sidecars are
// excluded (else ruby's article.rb + article.rbs pair is forever ambiguous).
function outputIndexForSource(srcPath) {
  if (!srcPath) return -1;
  const src = stemSegs(srcPath);
  if (!src.length) return -1;
  let cands = outFiles()
    .map((f, idx) => ({ idx, segs: stemSegs(f.path), sidecar: /\.(map|rbs)$/.test(f.path) }))
    .filter((o) => o.segs.length && !o.sidecar);
  for (let k = 1; k <= src.length; k++) {
    const suffix = src.slice(src.length - k); // the last k source segments
    const next = cands.filter((o) => endsWithSegs(o.segs, suffix));
    if (next.length === 0) return -1; // diverged: no output shares this suffix
    if (next.length === 1) return next[0].idx;
    cands = next; // still ambiguous → include one more parent dir
  }
  return -1; // exhausted the source path and it's still ambiguous
}

// The output-file picker is a popover tree (same widget as the source sidebar)
// instead of a flat dropdown, so a multi-dir emit (e.g. 79 TS files) is
// navigable. Output dirs default to expanded; `outClosedDirs` tracks collapses.
function renderOutTree() {
  const files = outFiles();
  renderTree(els.outfileMenu, files.map((f) => f.path), {
    isOpen: (d) => !outClosedDirs.has(d),
    toggleDir: (d) => { outClosedDirs.has(d) ? outClosedDirs.delete(d) : outClosedDirs.add(d); },
    isActive: (p) => files[currentOutIndex] && p === files[currentOutIndex].path,
    onPick: (p) => { showOutput(files.findIndex((f) => f.path === p)); closeOutMenu(); },
  });
}

function openOutMenu() { renderOutTree(); els.outfileMenu.hidden = false; }
function closeOutMenu() { els.outfileMenu.hidden = true; }

function renderOutput(result, ms) {
  closeOutMenu();
  if (!result || result.error) {
    setStatus(`error: ${result ? result.error : "no result"}`, "err");
    els.outfileBtn.textContent = "—";
    outputView.setValue(result && result.error ? result.error : "", "plaintext");
    return;
  }
  setStatus(`${result.language}: ${result.files.length} files${diagSummary(lastDiagnostics)} in ${ms.toFixed(1)} ms`, "ok");
  const oi = outputIndexForSource(currentPath); // keep the output on the open source's file
  showOutput(oi >= 0 ? oi : 0);
}

function showOutput(i) {
  const files = outFiles();
  if (!files[i]) return;
  // Same emitted file as before (an edit re-emitted it) → keep the scroll
  // position; a different file (source/output/target switch) → reset to top.
  const preserveView = files[i].path === currentOutPath;
  currentOutIndex = i;
  currentOutPath = files[i].path;
  els.outfileBtn.textContent = `${files[i].path}  (${files[i].content.length} B)`;
  outputView.setValue(files[i].content, OUT_LANG[els.target.value] || "plaintext", preserveView);
  if (!els.outfileMenu.hidden) renderOutTree(); // keep the open popover's highlight in sync
}

// ---- app bundles ---------------------------------------------------------
// apps.json (in ../lib/) lists the shipped apps; each `src` is either a flat
// {path:content} fixture (the blog seed) or a bundle-src.mjs output (which
// nests the map under `.src`). Swapping apps re-seeds srcMap and re-transpiles
// — the compiler is stateless per call. Ingest is survey-tolerant, so even
// Mastodon transpiles (partially, with a gap-note diagnostic on each unmodeled
// construct); the multi-second pass runs in the worker, so the UI stays live.
async function loadApp(entry) {
  setStatus(`loading ${entry.label || entry.name}…`);
  let json;
  try {
    json = await fetch(new URL(`../lib/${entry.src}`, import.meta.url)).then((r) => {
      if (!r.ok) throw new Error(r.status);
      return r.json();
    });
  } catch (e) {
    setStatus(`could not load ${entry.src}: ${e.message}`, "err");
    return;
  }
  srcMap = json && typeof json.src === "object" && json.src ? json.src : json;
  // Reset per-app view state; renderSources re-expands app/ for the new tree.
  openDirs = null;
  currentPath = null;
  currentOutIndex = 0;
  currentOutPath = null;
  outClosedDirs = new Set();
  lastOutput = null;
  lastDiagnostics = [];
  lastTypes = [];
  renderSources();
  const first = (entry.open && srcMap[entry.open] != null) ? entry.open
    : (srcMap[DEFAULT_FILE] != null ? DEFAULT_FILE : sourceFiles()[0]);
  selectFile(first);
  await transpile();
}

// ---- boot ----------------------------------------------------------------

async function boot() {
  for (const t of TARGETS) {
    const opt = document.createElement("option");
    opt.value = t;
    opt.textContent = t;
    els.target.appendChild(opt);
  }
  els.target.value = DEFAULT_TARGET;

  setStatus("loading wasm…");
  // The compiler wasm runs in the shared worker (lib/worker.mjs) behind a
  // watchdog: transpile never blocks the UI, and a hang/trap on a big app
  // restarts the worker instead of freezing the tab.
  client = createClient({
    workerUrl: new URL("../lib/worker.mjs", import.meta.url),
    wasmUrl: new URL("../lib/roundhouse_wasm.wasm", import.meta.url).href,
    timeoutMs: 30000,
    onRestart: (reason) => setStatus(`compiler restarted — ${reason}`, "err"),
  });
  await client.ready();

  setStatus("loading editor…");
  [editor, outputView] = await Promise.all([
    createEditor(els.editorHost, {
      onChange: onEditorChange,
      // Typed completion from the last transpile's analysis snapshot
      // (the wasm side stashes one on every transpile). `text` is the
      // live buffer — one keystroke ahead of that snapshot, which is
      // exactly the contract ide::complete_at is built for. Async now
      // (the round-trips through the worker); Monaco awaits it.
      complete: (text, line, character) =>
        currentPath ? client.complete(currentPath, text, line, character) : [],
    }),
    createOutputView(els.outputHost),
  ]);

  els.target.onchange = transpile;
  els.outfileBtn.onclick = () => els.outfileMenu.hidden ? openOutMenu() : closeOutMenu();
  // Click outside the picker closes the popover (clicks inside it — the button
  // toggle and the folder toggles — are left to their own handlers).
  document.addEventListener("click", (e) => {
    if (!els.outfileMenu.hidden && !els.picker.contains(e.target)) closeOutMenu();
  });

  // App picker. apps.json lists the shipped app bundles; absent (local dev
  // with only the lib fixture) → the lone blog fixture, selector hidden,
  // preserving the pre-picker behavior + the verify-playground harness.
  let manifest = null;
  try {
    manifest = await fetch(new URL("../lib/apps.json", import.meta.url)).then((r) => (r.ok ? r.json() : null));
  } catch { /* single-app dev */ }
  if (manifest && manifest.apps && manifest.apps.length) {
    apps = manifest.apps;
    for (const a of apps) {
      const opt = document.createElement("option");
      opt.value = a.name;
      opt.textContent = a.label || a.name;
      els.app.appendChild(opt);
    }
    const def = (manifest.default && apps.find((a) => a.name === manifest.default)) || apps[0];
    els.app.value = def.name;
    els.app.onchange = () => {
      const a = apps.find((x) => x.name === els.app.value);
      if (a) loadApp(a);
    };
    await loadApp(def);
  } else {
    els.appLabel.style.display = els.app.style.display = "none";
    await loadApp({ name: "blog", src: "fixture.json" });
  }

  // Programmatic hooks for the Playwright verifier — editor-widget agnostic.
  window.__playground = {
    ready: true,
    editorKind: editor.kind,
    apps: () => apps,
    async setApp(name) {
      const a = apps.find((x) => x.name === name);
      if (!a) return false;
      els.app.value = name;
      await loadApp(a);
      return true;
    },
    // Mutating hooks return the transpile promise so the verifier can await
    // the (now off-thread) re-transpile before reading output()/diagnostics().
    setTarget(t) { els.target.value = t; return transpile(); },
    selectSource: (path) => selectFile(path),
    editFile(path, content) {
      srcMap[path] = content;
      if (path === currentPath) editor.setValue(content, langForPath(path));
      return transpile();
    },
    output: () => lastOutput,
    // The emitted file currently shown in the output pane (path), for asserting
    // the source -> output follow behavior.
    displayedOutput: () => outFiles()[currentOutIndex]?.path ?? null,
    diagnostics: () => lastDiagnostics,
    types: () => lastTypes,
    // Smallest-span inferred type at a 1-based (line, col) in the open file.
    typeAt(line, col) {
      const hits = lastTypes.filter((t) => t.path === currentPath &&
        (line > t.start_line || (line === t.start_line && col >= t.start_col)) &&
        (line < t.end_line || (line === t.end_line && col <= t.end_col)));
      hits.sort((a, b) =>
        ((a.end_line - a.start_line) * 1e5 + (a.end_col - a.start_col)) -
        ((b.end_line - b.start_line) * 1e5 + (b.end_col - b.start_col)));
      return hits.length ? hits[0].ty : null;
    },
    source: (path) => srcMap[path],
    sourceCount: () => sourceFiles().length,
    // Typed completion at a 0-based (line, character) against `text` as the
    // current buffer for the open file — drives the completion assertion in
    // the verifier without simulating editor keystrokes.
    complete: (text, line, character) =>
      client.complete(currentPath, text, line, character),
  };
}

boot().catch((e) => setStatus(`boot failed: ${e.message}`, "err"));
