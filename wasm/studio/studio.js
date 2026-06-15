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

const TARGET = "typescript"; // studio is TS-only — the only browser runtime
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
let editor = null;
let srcMap = null;          // { path: content } — the live, editable input
let currentPath = null;     // which source file the editor is showing
let openDirs = null;        // Set<string> of expanded directory paths
let lastBuild = null;       // { files, ms, error, diagnostics }
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

function build() {
  const t0 = performance.now();
  let out;
  try {
    out = compiler.transpile(TARGET, srcMap);
  } catch (e) {
    out = { error: `transpile threw: ${e.message}` };
  }
  const ms = performance.now() - t0;
  lastBuild = {
    files: out.files || [],
    diagnostics: out.diagnostics || [],
    error: out.error || null,
    ms,
  };
  renderApp();
  renderMarkers();
}

function renderMarkers() {
  if (!editor || !lastBuild) return;
  editor.setMarkers(lastBuild.diagnostics.filter((d) => d.path === currentPath));
}

// ---- app pane (Phase 4: build readout + Phase 5 roadmap) -----------------

function renderApp() {
  const b = lastBuild || { files: [], ms: 0, error: null, diagnostics: [] };
  const errs = b.diagnostics.filter((d) => d.severity === "error").length;
  if (b.error) {
    setStatus(`build error: ${b.error}`, "err");
  } else {
    setStatus(`compiled ${b.files.length} TS files${errs ? ` · ${errs} error${errs > 1 ? "s" : ""}` : ""} in ${b.ms.toFixed(1)} ms`, errs ? "err" : "ok");
  }

  els.appHost.innerHTML = "";
  const h = document.createElement("h2");
  h.textContent = "Live app loop — coming in Phase 5";
  const p = document.createElement("p");
  p.innerHTML =
    "Studio compiles your Ruby to TypeScript in the browser on every edit " +
    "(see the build readout below — it's live). The next step bundles that TS " +
    "with <code>esbuild-wasm</code> and hot-swaps it into the running blog " +
    "(SharedWorker + sqlite-wasm), so this pane becomes the app itself.";
  const road = document.createElement("p");
  road.className = "roadmap";
  road.innerHTML =
    "Until then: open <a href=\"../blog/\">/blog/</a> to see that running " +
    "runtime, or <a href=\"../playground/\">/playground/</a> to read the " +
    "emitted code for every target.";

  const build = document.createElement("div");
  build.id = "buildline";
  if (b.error) {
    build.innerHTML = `<span class="k">last build</span> <span style="color:#b00020">${escapeHtml(b.error)}</span>`;
  } else {
    build.innerHTML =
      `<span class="k">last build</span> ${TARGET} · ${b.files.length} files · ` +
      `${b.ms.toFixed(1)} ms · ${errs} error${errs === 1 ? "" : "s"}`;
  }

  els.appHost.append(h, p, road, build);
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
  setStatus("loading editor…");
  editor = await createEditor(els.editorHost, { onChange: onEditorChange });

  const first = srcMap[DEFAULT_FILE] != null ? DEFAULT_FILE : sourceFiles()[0];
  selectFile(first);
  build();

  // Programmatic hooks for the Playwright verifier — editor-widget agnostic.
  window.__studio = {
    ready: true,
    editorKind: editor.kind,
    selectSource: (path) => selectFile(path),
    editFile(path, content) {
      srcMap[path] = content;
      if (path === currentPath) editor.setValue(content, langForPath(path));
      build();
    },
    build: () => lastBuild,
    source: (path) => srcMap[path],
    sourceCount: () => sourceFiles().length,
  };
}

boot().catch((e) => setStatus(`boot failed: ${e.message}`, "err"));
