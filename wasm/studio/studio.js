// roundhouse studio — rung D of docs/browser-demo-plan.md.
//
// "The blog, editable": edit Ruby → recompile in-browser → run the emitted
// TypeScript blog live against a sqlite-wasm DB. TypeScript-only (the one
// browser runtime; decision #5/#6), so there is no target dropdown.
//
// This is the Phase 4 SCAFFOLD. It proves the shared ../lib/ on a second
// surface — the editor, the source tree, and the debounced edit→transpile
// loop all work here — and wires the transpile step end to end (the build
// readout in the app pane is live). What it does NOT yet do is RUN the app:
// that's Phase 5 (esbuild-wasm bundle of the emitted TS + hot-swap the blog's
// SharedWorker/sqlite-wasm runtime). The app pane shows that roadmap plus the
// live build status, so the loop is demonstrably half-built, not faked.
//
// Serve the PARENT (wasm/) as the web root so /studio/ and /lib/ both resolve:
//   python3 -m http.server 8099   # run from wasm/
//   open http://localhost:8099/studio/

import { loadDefaultCompiler, loadFixture } from "../lib/transpile.mjs";
import { createEditor } from "../lib/editor.js";
import { allDirPaths, renderTree } from "../lib/tree.js";
import { loadBundler } from "../lib/bundle.mjs";

const TARGET = "typescript"; // studio is TS-only — the only browser runtime
const PROFILE = "worker";    // the SharedWorker browser app (what studio runs)
const DEBOUNCE_MS = 250;
const DEFAULT_FILE = "app/views/articles/index.html.erb"; // a view: most visibly "live"

const els = {
  status: document.getElementById("status"),
  srcfiles: document.getElementById("srcfiles"),
  editorHost: document.getElementById("editorHost"),
  editorHead: document.getElementById("editorHead"),
  appHost: document.getElementById("appHost"),
};

let compiler = null;
let bundler = null;         // esbuild-wasm bundler (null if its CDN load failed)
let editor = null;
let srcMap = null;          // { path: content } — the live, editable input (Ruby)
let currentPath = null;     // which source file the editor is showing
let openDirs = null;        // Set<string> of expanded directory paths
let lastBuild = null;       // { files, transpileMs, error, diagnostics }
let lastBundle = null;      // { ms, errors, warnings, outputs } from esbuild
let buildSeq = 0;           // guards against a stale async bundle rendering late
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
  const paths = sourceFiles();
  // First render: expand the app/ subtree (the interesting code), collapse the
  // rest. Subsequent renders preserve the user's toggles.
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
  renderMarkers();
}

// ---- transpile loop ------------------------------------------------------

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
  lastBundle = null; // invalidate until the new bundle lands
  renderApp();
  renderMarkers();

  // 2. Bundle the emitted TS → browser-loadable ESM (esbuild-wasm). Async; a
  // newer edit (higher seq) supersedes this one's result.
  if (out.error || !bundler) return;
  const emitted = Object.fromEntries(lastBuild.files.map((f) => [f.path, f.content]));
  let b;
  try {
    b = await bundler.bundle(emitted);
  } catch (e) {
    b = { ms: 0, errors: [{ text: `bundler threw: ${e.message}` }], warnings: [], outputs: {} };
  }
  if (seq !== buildSeq) return; // a later build already ran
  lastBundle = b;
  renderApp();
}

function renderMarkers() {
  if (!editor || !lastBuild) return;
  editor.setMarkers(lastBuild.diagnostics.filter((d) => d.path === currentPath));
}

// ---- app pane (Phase 4: transpile + esbuild bundle readout) --------------

function renderApp() {
  const b = lastBuild || { files: [], transpileMs: 0, error: null, diagnostics: [] };
  const errs = b.diagnostics.filter((d) => d.severity === "error").length;
  if (b.error) {
    setStatus(`build error: ${b.error}`, "err");
  } else {
    const bundleNote = lastBundle
      ? (lastBundle.errors.length ? ` · bundle: ${lastBundle.errors.length} err` : ` · bundled ${Object.keys(lastBundle.outputs).length} ESM`)
      : (bundler ? " · bundling…" : "");
    setStatus(`compiled ${b.files.length} TS files${errs ? ` · ${errs} error${errs > 1 ? "s" : ""}` : ""} in ${b.transpileMs.toFixed(1)} ms${bundleNote}`, errs ? "err" : "ok");
  }

  els.appHost.innerHTML = "";
  const h = document.createElement("h2");
  h.textContent = "Live app loop — Phase 5 (next)";
  const p = document.createElement("p");
  p.innerHTML =
    "On every edit, studio compiles your Ruby to the worker-profile TypeScript " +
    "and bundles it to browser-loadable ESM entirely client-side — both steps " +
    "are live below. Phase 5 loads those three bundles as the running app " +
    "(main thread + SharedWorker + DB worker) over sqlite-wasm, and hot-swaps " +
    "them so this pane becomes the app itself.";
  const road = document.createElement("p");
  road.className = "roadmap";
  road.innerHTML =
    "Meanwhile: open <a href=\"../blog/\">/blog/</a> to see that running " +
    "runtime, or <a href=\"../playground/\">/playground/</a> to read the " +
    "emitted code for every target.";

  els.appHost.append(h, p, road, transpileLine(b, errs), bundleLine());
}

function transpileLine(b, errs) {
  const el = document.createElement("div");
  el.id = "buildline";
  el.innerHTML = b.error
    ? `<span class="k">transpile</span> <span style="color:#b00020">${escapeHtml(b.error)}</span>`
    : `<span class="k">transpile</span> ${TARGET}/${PROFILE} · ${b.files.length} files · ` +
      `${b.transpileMs.toFixed(1)} ms · ${errs} error${errs === 1 ? "" : "s"}`;
  return el;
}

function bundleLine() {
  const el = document.createElement("div");
  el.id = "bundleline";
  if (!bundler) {
    el.innerHTML = `<span class="k">bundle</span> esbuild-wasm unavailable (offline?) — transpile still live`;
    return el;
  }
  if (!lastBundle) {
    el.innerHTML = `<span class="k">bundle</span> …`;
    return el;
  }
  if (lastBundle.errors.length) {
    el.innerHTML = `<span class="k">bundle</span> <span style="color:#b00020">${escapeHtml(lastBundle.errors[0].text)}</span>`;
    return el;
  }
  const sizes = Object.entries(lastBundle.outputs)
    .map(([name, o]) => `${name} ${(o.bytes / 1024).toFixed(1)}KB`)
    .join(" · ");
  el.innerHTML = `<span class="k">bundle</span> ${sizes} · ${lastBundle.ms.toFixed(0)} ms <span style="color:#176e2b">(ESM, ready to run)</span>`;
  return el;
}

function escapeHtml(s) {
  return s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
}

// ---- boot ----------------------------------------------------------------

async function boot() {
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
  setStatus("loading editor + bundler…");
  // Editor and esbuild load in parallel; if esbuild's CDN load fails (offline /
  // strict CSP), studio degrades to transpile-only rather than failing to boot.
  const [ed, bnd] = await Promise.all([
    createEditor(els.editorHost, { onChange: onEditorChange }),
    loadBundler().catch((e) => { console.warn("[studio] bundler unavailable:", e.message); return null; }),
  ]);
  editor = ed;
  bundler = bnd;

  const first = srcMap[DEFAULT_FILE] != null ? DEFAULT_FILE : sourceFiles()[0];
  selectFile(first);
  build();

  // Programmatic hooks for the Playwright verifier — editor-widget agnostic.
  window.__studio = {
    ready: true,
    editorKind: editor.kind,
    hasBundler: () => bundler != null,
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
