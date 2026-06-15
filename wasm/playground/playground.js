// Phase 1 — multi-target playground (rung A of docs/browser-demo-plan.md).
//
// Self-contained: every asset it needs (the C-ABI driver transpile.mjs +
// wasi-shim.mjs, the compiler roundhouse_wasm.wasm, and the seed app
// fixture.json) sits in THIS directory, so the whole dir copies straight to
// GitHub Pages at /playground/ with no rewrite. transpile.mjs + wasi-shim.mjs
// are vendored copies of the Phase 0 spike's driver (kept in sync by hand —
// they're small and stable). What's net-new over the spike: an EDITABLE
// source tree, an editor (Monaco w/ textarea fallback), and a debounced
// edit -> transpile -> render loop.
//
// Serve THIS directory as the web root (e.g. `python3 -m http.server` here).

import { loadCompiler } from "./transpile.mjs";
import { createEditor } from "./editor.js";

// The six targets the wasm entry point routes to today. (ruby/spinel/kotlin/
// swift are not yet wired into wasm/src/lib.rs — a one-line match extension.)
const TARGETS = ["typescript", "go", "rust", "python", "elixir", "crystal"];

// Map a target to a Monaco-ish language id for the (read-only) output pane.
// Elixir/Crystal have no built-in Monaco grammar -> plaintext.
const OUT_LANG = {
  typescript: "typescript", go: "go", rust: "rust",
  python: "python", elixir: "plaintext", crystal: "plaintext",
};

const DEBOUNCE_MS = 250;
const DEFAULT_FILE = "app/models/article.rb";

const els = {
  target: document.getElementById("target"),
  status: document.getElementById("status"),
  srcfiles: document.getElementById("srcfiles"),
  editorHost: document.getElementById("editorHost"),
  editorHead: document.getElementById("editorHead"),
  outfiles: document.getElementById("outfiles"),
  outcode: document.querySelector("#outcode code"),
};

let compiler = null;
let editor = null;
let srcMap = null;          // { path: content } — the live, editable input
let currentPath = null;     // which source file the editor is showing
let lastOutput = null;      // last { language, files } | { error }
let lastDiagnostics = [];   // last result's diagnostics (target-independent)
let debounceTimer = null;

function setStatus(msg, kind = "") {
  els.status.textContent = msg;
  els.status.className = kind;
}

// ---- source tree ---------------------------------------------------------

function sourceFiles() {
  return Object.keys(srcMap).filter((p) => /\.(rb|erb)$/.test(p)).sort();
}

function renderSources() {
  els.srcfiles.innerHTML = "";
  for (const path of sourceFiles()) {
    const li = document.createElement("li");
    const btn = document.createElement("button");
    btn.textContent = path.replace(/^app\//, "");
    btn.title = path;
    btn.dataset.path = path;
    btn.onclick = () => selectFile(path);
    li.appendChild(btn);
    els.srcfiles.appendChild(li);
  }
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
  renderOutput(lastOutput, performance.now() - t0);
  renderMarkers();
}

// Inference diagnostics (target-independent): squiggles on the open file +
// a count in the status bar. Editing to introduce a type error (e.g.
// `title + 1`) surfaces a red marker live — the inference-first demo.
function renderMarkers() {
  if (!editor) return;
  editor.setMarkers(lastDiagnostics.filter((d) => d.path === currentPath));
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

function renderOutput(result, ms) {
  els.outfiles.innerHTML = "";
  if (!result || result.error) {
    setStatus(`error: ${result ? result.error : "no result"}`, "err");
    els.outcode.textContent = result && result.error ? result.error : "";
    return;
  }
  setStatus(`${result.language}: ${result.files.length} files${diagSummary(lastDiagnostics)} in ${ms.toFixed(1)} ms`, "ok");
  result.files.forEach((f, i) => {
    const li = document.createElement("li");
    const btn = document.createElement("button");
    btn.textContent = `${f.path}  (${f.content.length} B)`;
    btn.onclick = () => showOutput(i);
    li.appendChild(btn);
    els.outfiles.appendChild(li);
  });
  showOutput(0);
}

function showOutput(i) {
  if (!lastOutput || !lastOutput.files) return;
  els.outcode.textContent = lastOutput.files[i].content;
  [...els.outfiles.querySelectorAll("button")].forEach((b, j) =>
    b.classList.toggle("active", j === i));
}

// ---- boot ----------------------------------------------------------------

async function boot() {
  for (const t of TARGETS) {
    const opt = document.createElement("option");
    opt.value = t;
    opt.textContent = t;
    els.target.appendChild(opt);
  }

  setStatus("loading wasm + fixture…");
  const [wasmBytes, fixture] = await Promise.all([
    fetch("./roundhouse_wasm.wasm").then((r) => r.arrayBuffer()),
    fetch("./fixture.json").then((r) => r.json()),
  ]);
  srcMap = fixture;
  compiler = await loadCompiler(wasmBytes, {
    onStdout: (s) => console.log("[wasm]", s),
    onStderr: (s) => console.warn("[wasm]", s),
  });

  renderSources();
  setStatus("loading editor…");
  editor = await createEditor(els.editorHost, { onChange: onEditorChange });

  const first = srcMap[DEFAULT_FILE] != null ? DEFAULT_FILE : sourceFiles()[0];
  selectFile(first);
  els.target.onchange = transpile;
  transpile();

  // Programmatic hooks for the Playwright verifier — editor-widget agnostic.
  window.__playground = {
    ready: true,
    editorKind: editor.kind,
    setTarget(t) { els.target.value = t; transpile(); },
    editFile(path, content) {
      srcMap[path] = content;
      if (path === currentPath) editor.setValue(content, langForPath(path));
      transpile();
    },
    output: () => lastOutput,
    diagnostics: () => lastDiagnostics,
    source: (path) => srcMap[path],
    sourceCount: () => sourceFiles().length,
  };
}

boot().catch((e) => setStatus(`boot failed: ${e.message}`, "err"));
