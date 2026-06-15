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
import { createEditor, createOutputView } from "./editor.js";

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
  outfile: document.getElementById("outfile"),
  outputHost: document.getElementById("outputHost"),
};

let compiler = null;
let editor = null;
let outputView = null;      // read-only Monaco (or <pre>) showing the emitted file
let srcMap = null;          // { path: content } — the live, editable input
let currentPath = null;     // which source file the editor is showing
let openDirs = null;        // Set<string> of expanded directory paths in the tree
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

// Build a nested {dirs: Map<name,node>, files: [{name,path}]} tree from the
// flat, slash-delimited source paths.
function buildTree(paths) {
  const root = { dirs: new Map(), files: [] };
  for (const path of paths) {
    const parts = path.split("/");
    let node = root;
    for (let i = 0; i < parts.length - 1; i++) {
      if (!node.dirs.has(parts[i])) node.dirs.set(parts[i], { dirs: new Map(), files: [] });
      node = node.dirs.get(parts[i]);
    }
    node.files.push({ name: parts[parts.length - 1], path });
  }
  return root;
}

// Every interior directory path (e.g. "app", "app/views", "app/views/articles").
function allDirPaths(paths) {
  const dirs = new Set();
  for (const path of paths) {
    const parts = path.split("/");
    for (let i = 1; i < parts.length; i++) dirs.add(parts.slice(0, i).join("/"));
  }
  return dirs;
}

function renderTreeLevel(node, prefix) {
  const ul = document.createElement("ul");
  ul.className = "tree";
  for (const [name, child] of [...node.dirs.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
    const dirPath = prefix ? `${prefix}/${name}` : name;
    const open = openDirs.has(dirPath);
    const li = document.createElement("li");
    const btn = document.createElement("button");
    btn.className = "folder";
    btn.innerHTML = `<span class="tw">${open ? "▾" : "▸"}</span>`;
    btn.append(`${name}/`);
    btn.onclick = () => { open ? openDirs.delete(dirPath) : openDirs.add(dirPath); renderSources(); };
    li.appendChild(btn);
    if (open) li.appendChild(renderTreeLevel(child, dirPath));
    ul.appendChild(li);
  }
  for (const f of node.files.sort((a, b) => a.name.localeCompare(b.name))) {
    const li = document.createElement("li");
    const btn = document.createElement("button");
    btn.className = "file";
    btn.textContent = f.name;
    btn.title = f.path;
    btn.dataset.path = f.path;
    btn.classList.toggle("active", f.path === currentPath);
    btn.onclick = () => selectFile(f.path);
    li.appendChild(btn);
    ul.appendChild(li);
  }
  return ul;
}

function renderSources() {
  const paths = sourceFiles();
  // First render: expand the app/ subtree (the interesting code), collapse the
  // rest (config/db/test). Subsequent renders preserve the user's toggles.
  if (openDirs === null) {
    openDirs = new Set([...allDirPaths(paths)].filter((d) => d === "app" || d.startsWith("app/")));
  }
  els.srcfiles.innerHTML = "";
  els.srcfiles.appendChild(renderTreeLevel(buildTree(paths), ""));
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

function renderOutput(result, ms) {
  els.outfile.innerHTML = "";
  if (!result || result.error) {
    setStatus(`error: ${result ? result.error : "no result"}`, "err");
    outputView.setValue(result && result.error ? result.error : "", "plaintext");
    return;
  }
  setStatus(`${result.language}: ${result.files.length} files${diagSummary(lastDiagnostics)} in ${ms.toFixed(1)} ms`, "ok");
  result.files.forEach((f, i) => {
    const opt = document.createElement("option");
    opt.value = String(i);
    opt.textContent = `${f.path}  (${f.content.length} B)`;
    els.outfile.appendChild(opt);
  });
  showOutput(0);
}

function showOutput(i) {
  if (!lastOutput || !lastOutput.files) return;
  els.outfile.value = String(i);
  outputView.setValue(lastOutput.files[i].content, OUT_LANG[els.target.value] || "plaintext");
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
  [editor, outputView] = await Promise.all([
    createEditor(els.editorHost, { onChange: onEditorChange }),
    createOutputView(els.outputHost),
  ]);

  const first = srcMap[DEFAULT_FILE] != null ? DEFAULT_FILE : sourceFiles()[0];
  selectFile(first);
  els.target.onchange = transpile;
  els.outfile.onchange = () => showOutput(Number(els.outfile.value));
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
  };
}

boot().catch((e) => setStatus(`boot failed: ${e.message}`, "err"));
