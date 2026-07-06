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

import { loadDefaultCompiler, loadFixture } from "../lib/transpile.mjs";
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
  status: document.getElementById("status"),
  srcfiles: document.getElementById("srcfiles"),
  editorHost: document.getElementById("editorHost"),
  editorHead: document.getElementById("editorHead"),
  picker: document.getElementById("outpicker"),
  outfileBtn: document.getElementById("outfileBtn"),
  outfileMenu: document.getElementById("outfileMenu"),
  outputHost: document.getElementById("outputHost"),
};

let compiler = null;
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

function transpile() {
  const lang = els.target.value;
  const t0 = performance.now();
  try {
    lastOutput = compiler.transpile(lang, srcMap);
  } catch (e) {
    lastOutput = { error: `transpile threw: ${e.message}` };
  }
  lastDiagnostics = (lastOutput && lastOutput.diagnostics) || [];
  lastTypes = (lastOutput && lastOutput.inferred_types) || [];
  renderOutput(lastOutput, performance.now() - t0);
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

// ---- boot ----------------------------------------------------------------

async function boot() {
  for (const t of TARGETS) {
    const opt = document.createElement("option");
    opt.value = t;
    opt.textContent = t;
    els.target.appendChild(opt);
  }
  els.target.value = DEFAULT_TARGET;

  setStatus("loading wasm + fixture…");
  const [loaded, fixture] = await Promise.all([
    loadDefaultCompiler({
      onStdout: (s) => console.log("[wasm]", s),
      onStderr: (s) => console.warn("[wasm]", s),
    }),
    loadFixture(),
  ]);
  compiler = loaded;
  srcMap = fixture;

  renderSources();
  setStatus("loading editor…");
  [editor, outputView] = await Promise.all([
    createEditor(els.editorHost, {
      onChange: onEditorChange,
      // Typed completion from the last transpile's analysis snapshot
      // (the wasm side stashes one on every transpile). `text` is the
      // live buffer — one keystroke ahead of that snapshot, which is
      // exactly the contract ide::complete_at is built for.
      complete: (text, line, character) =>
        currentPath ? compiler.complete(currentPath, text, line, character) : [],
    }),
    createOutputView(els.outputHost),
  ]);

  const first = srcMap[DEFAULT_FILE] != null ? DEFAULT_FILE : sourceFiles()[0];
  selectFile(first);
  els.target.onchange = transpile;
  els.outfileBtn.onclick = () => els.outfileMenu.hidden ? openOutMenu() : closeOutMenu();
  // Click outside the picker closes the popover (clicks inside it — the button
  // toggle and the folder toggles — are left to their own handlers).
  document.addEventListener("click", (e) => {
    if (!els.outfileMenu.hidden && !els.picker.contains(e.target)) closeOutMenu();
  });
  transpile();

  // Programmatic hooks for the Playwright verifier — editor-widget agnostic.
  window.__playground = {
    ready: true,
    editorKind: editor.kind,
    setTarget(t) { els.target.value = t; transpile(); },
    selectSource: (path) => selectFile(path),
    editFile(path, content) {
      srcMap[path] = content;
      if (path === currentPath) editor.setValue(content, langForPath(path));
      transpile();
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
      compiler.complete(currentPath, text, line, character),
  };
}

boot().catch((e) => setStatus(`boot failed: ${e.message}`, "err"));
