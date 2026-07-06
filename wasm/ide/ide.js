// The /ide/ page: Monaco over the roundhouse analyzer running in a Web
// Worker, preloaded with a real Rails app (the published site ships
// Mastodon at a pinned SHA; see app-src.json). Everything typed here is
// answered by whole-program inference — no server, no boot, no
// annotations:
//
//   hover        → inferred type at the cursor (worker type_at)
//   completion   → members/kwargs/ivars from the last-good snapshot
//   markers      → diagnostics with coverage notes (info severity =
//                  "roundhouse can't see this yet", not an app error)
//   ⌘P / Ctrl+P  → fuzzy file picker (also searches classes)
//   ⌘⇧R          → related files (inferred render graph + includes)
//
// Edits re-analyze in the worker (debounced); queries answer from the
// previous snapshot meanwhile — stale-by-one-edit, the standard
// fast-language-server trade.

import { loadMonaco } from "../lib/editor.js";
import { buildTree, allDirPaths, renderTree } from "../lib/tree.js";

const els = {
  status: document.getElementById("status"),
  counts: document.getElementById("counts"),
  tree: document.getElementById("tree"),
  editor: document.getElementById("editor"),
  editorHead: document.getElementById("editorHead"),
  picker: document.getElementById("picker"),
  pickerInput: document.getElementById("pickerInput"),
  pickerList: document.getElementById("pickerList"),
};

// ── Worker RPC ───────────────────────────────────────────────────────
const worker = new Worker(new URL("./worker.mjs", import.meta.url), { type: "module" });
let nextId = 1;
const pending = new Map();
worker.onmessage = (e) => {
  const { id, result, error } = e.data;
  const p = pending.get(id);
  if (!p) return;
  pending.delete(id);
  error ? p.reject(new Error(error)) : p.resolve(result);
};
function rpc(op, args) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    worker.postMessage({ id, op, args });
  });
}

// ── State ────────────────────────────────────────────────────────────
let srcMap = {};             // path → current text (the edit overlay)
let analysis = null;         // last AnalyzeOutput
let monaco = null;
let editor = null;
const models = new Map();    // path → monaco model
const modelPath = new Map(); // model → path
let activePath = null;
const openDirs = new Set(["app", "app/models", "app/controllers", "app/views"]);
const mru = [];

function status(text) { els.status.textContent = text; }

function langFor(path) {
  if (path.endsWith(".erb") || path.endsWith(".haml")) return "html";
  if (path.endsWith(".rb") || path.endsWith(".jbuilder") || path.endsWith(".ruby")) return "ruby";
  return "plaintext";
}

// ── Analysis loop ────────────────────────────────────────────────────
let analyzing = false;
let dirty = false;
async function reanalyze() {
  if (analyzing) { dirty = true; return; }
  analyzing = true;
  status("analyzing…");
  try {
    const result = await rpc("analyze", { src: srcMap });
    if (result.error) {
      status(`analysis error: ${result.error}`);
    } else {
      analysis = result;
      const sev = { error: 0, warning: 0, info: 0 };
      for (const d of analysis.diagnostics) sev[d.severity] = (sev[d.severity] || 0) + 1;
      els.counts.textContent =
        `${sev.error} errors · ${sev.warning} warnings · ${sev.info} coverage notes · ` +
        `${analysis.gaps.length} ingest gaps`;
      status(`analyzed ${analysis.files.length} files in ${(result.elapsed_ms / 1000).toFixed(1)}s`);
      refreshAllMarkers();
    }
  } catch (err) {
    status(`analysis failed: ${err.message}`);
  }
  analyzing = false;
  if (dirty) { dirty = false; reanalyze(); }
}
let debounceTimer = null;
function scheduleReanalyze() {
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(reanalyze, 1500);
}

// ── Markers ──────────────────────────────────────────────────────────
// The analyzer's paths and the picker's are the same (srcMap keys were
// the ingest tree), so grouping is a straight bucket.
function refreshAllMarkers() {
  if (!analysis || !monaco) return;
  const byPath = new Map();
  for (const d of analysis.diagnostics) {
    if (!byPath.has(d.path)) byPath.set(d.path, []);
    byPath.get(d.path).push(d);
  }
  for (const [path, model] of models) {
    const diags = byPath.get(path) || [];
    monaco.editor.setModelMarkers(model, "roundhouse", diags.map((d) => ({
      startLineNumber: d.start_line, startColumn: d.start_col,
      endLineNumber: d.end_line, endColumn: d.end_col,
      message: `[${d.code}] ${d.message}`,
      severity: d.severity === "error" ? monaco.MarkerSeverity.Error
        : d.severity === "warning" ? monaco.MarkerSeverity.Warning
        : monaco.MarkerSeverity.Hint,
    })));
  }
}

// ── Files ────────────────────────────────────────────────────────────
function openFile(path) {
  if (!(path in srcMap)) return;
  activePath = path;
  const i = mru.indexOf(path);
  if (i >= 0) mru.splice(i, 1);
  mru.unshift(path);
  let model = models.get(path);
  if (!model) {
    model = monaco.editor.createModel(srcMap[path], langFor(path));
    models.set(path, model);
    modelPath.set(model, path);
    model.onDidChangeContent(() => {
      srcMap[path] = model.getValue();
      scheduleReanalyze();
    });
  }
  editor.setModel(model);
  els.editorHead.textContent = path;
  refreshAllMarkers();
  redrawTree();
  editor.focus();
}

function redrawTree() {
  renderTree(els.tree, Object.keys(srcMap).sort(), {
    isOpen: (dir) => openDirs.has(dir),
    toggleDir: (dir) => { openDirs.has(dir) ? openDirs.delete(dir) : openDirs.add(dir); redrawTree(); },
    isActive: (path) => path === activePath,
    onPick: openFile,
  });
}

// ── Fuzzy picker (files + classes + related) ─────────────────────────
// Subsequence match, scored: consecutive hits and basename hits rank
// higher; MRU files float. Classes appear as `class · Name` rows and
// resolve through the analyzer's related/registry paths.
function fuzzyScore(needle, hay) {
  needle = needle.toLowerCase();
  const hayLower = hay.toLowerCase();
  let hi = 0, score = 0, run = 0;
  for (const ch of needle) {
    const idx = hayLower.indexOf(ch, hi);
    if (idx < 0) return -1;
    run = idx === hi ? run + 1 : 1;
    score += run * 2 + (hay.lastIndexOf("/") < idx ? 2 : 0);
    hi = idx + 1;
  }
  return score - hay.length / 100;
}

let pickerItems = [];
let pickerSel = 0;
function showPicker(items, placeholder) {
  pickerItems = items;
  pickerSel = 0;
  els.pickerInput.value = "";
  els.pickerInput.placeholder = placeholder;
  els.picker.style.display = "block";
  drawPicker("");
  els.pickerInput.focus();
}
function hidePicker() { els.picker.style.display = "none"; editor?.focus(); }
function drawPicker(query) {
  let rows;
  if (query) {
    rows = pickerItems
      .map((it) => ({ it, s: fuzzyScore(query, it.search) }))
      .filter((r) => r.s >= 0)
      .sort((a, b) => b.s - a.s)
      .slice(0, 40)
      .map((r) => r.it);
  } else {
    rows = pickerItems.slice(0, 40);
  }
  pickerSel = Math.min(pickerSel, Math.max(0, rows.length - 1));
  els.pickerList.innerHTML = "";
  rows.forEach((it, i) => {
    const li = document.createElement("li");
    li.textContent = it.label;
    if (it.hint) {
      const span = document.createElement("span");
      span.className = "hint";
      span.textContent = it.hint;
      li.appendChild(span);
    }
    if (i === pickerSel) li.classList.add("sel");
    li.onclick = () => { hidePicker(); it.run(); };
    els.pickerList.appendChild(li);
  });
  els.pickerList._rows = rows;
}
els.pickerInput.addEventListener("input", () => { pickerSel = 0; drawPicker(els.pickerInput.value); });
els.pickerInput.addEventListener("keydown", (e) => {
  const rows = els.pickerList._rows || [];
  if (e.key === "Escape") { hidePicker(); }
  else if (e.key === "ArrowDown") { pickerSel = Math.min(pickerSel + 1, rows.length - 1); drawPicker(els.pickerInput.value); e.preventDefault(); }
  else if (e.key === "ArrowUp") { pickerSel = Math.max(pickerSel - 1, 0); drawPicker(els.pickerInput.value); e.preventDefault(); }
  else if (e.key === "Enter" && rows[pickerSel]) { hidePicker(); rows[pickerSel].run(); }
});

function fileItems() {
  const paths = Object.keys(srcMap);
  const ordered = [...mru, ...paths.filter((p) => !mru.includes(p))];
  const items = ordered.map((p) => ({ label: p, search: p, run: () => openFile(p) }));
  for (const c of analysis?.classes || []) {
    items.push({
      label: c, hint: "class", search: c,
      run: async () => {
        // Resolve a class to its file through related_files' index by
        // asking for the class's own related set is indirect; instead
        // find a source whose path matches the conventional location,
        // falling back to a text search across paths.
        const snake = c.replace(/::/g, "/").replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
        const hit = Object.keys(srcMap).find((p) => p.endsWith(`${snake}.rb`));
        if (hit) openFile(hit);
      },
    });
  }
  return items;
}

async function showRelated() {
  if (!activePath) return;
  const rel = await rpc("related", { path: activePath });
  if (rel.error || !rel.length) { status(rel.error || "no related files known"); return; }
  showPicker(
    rel.map((r) => ({
      label: `${r.kind} · ${r.label}`,
      search: `${r.kind} ${r.label}`,
      run: () => openFile(r.path),
    })),
    `related to ${activePath}`,
  );
}

// ── Boot ─────────────────────────────────────────────────────────────
async function boot() {
  status("loading editor…");
  monaco = await loadMonaco();

  editor = monaco.editor.create(els.editor, {
    value: "", language: "ruby", automaticLayout: true,
    minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false,
    tabSize: 2,
  });

  // Hover: inferred type from the last-good snapshot.
  monaco.languages.registerHoverProvider(["ruby", "html"], {
    async provideHover(model, position) {
      const path = modelPath.get(model);
      if (!path) return null;
      const info = await rpc("typeAt", {
        path, line: position.lineNumber - 1, character: position.column - 1,
      });
      if (!info || info.error) return null;
      const nil = info.nilable ? "\n\nMay be `nil`." : "";
      return { contents: [{ value: "```ruby\n" + info.display + "\n```" + nil }] };
    },
  });

  // Completion: the shared ide::complete_at core, via the worker.
  const kindMap = () => ({
    column: monaco.languages.CompletionItemKind.Field,
    kwarg: monaco.languages.CompletionItemKind.Field,
    association: monaco.languages.CompletionItemKind.Property,
    accessor: monaco.languages.CompletionItemKind.Property,
    scope: monaco.languages.CompletionItemKind.Function,
    method: monaco.languages.CompletionItemKind.Method,
    ivar: monaco.languages.CompletionItemKind.Variable,
  });
  monaco.languages.registerCompletionItemProvider(["ruby", "html"], {
    triggerCharacters: [".", "@", "(", ",", " "],
    async provideCompletionItems(model, position) {
      const path = modelPath.get(model);
      if (!path) return { suggestions: [] };
      const cands = await rpc("complete", {
        path,
        text: model.getValue(),
        line: position.lineNumber - 1,
        character: position.column - 1,
      });
      if (!Array.isArray(cands)) return { suggestions: [] };
      const kinds = kindMap();
      const word = model.getWordUntilPosition(position);
      const range = new monaco.Range(
        position.lineNumber, word.startColumn, position.lineNumber, word.endColumn,
      );
      return {
        suggestions: cands.map((c) => ({
          label: { label: c.label, description: c.detail },
          kind: kinds[c.kind] ?? kinds.method,
          detail: c.detail,
          sortText: c.sort_text,
          insertText: c.insert_text || c.label,
          range,
        })),
      };
    },
  });

  status("loading sources…");
  const resp = await fetch("./app-src.json");
  if (!resp.ok) {
    status("no app-src.json — generate one with: node bundle-src.mjs <rails-app-root>");
    return;
  }
  const bundle = await resp.json();
  srcMap = bundle.src;
  document.title = `roundhouse ide — ${bundle.name || "rails app"}`;
  redrawTree();

  status("loading analyzer…");
  await rpc("init", { wasmUrl: new URL("../lib/roundhouse_wasm.wasm", import.meta.url).href });
  await reanalyze();

  // Open a sensible first file.
  const first = bundle.open || Object.keys(srcMap).find((p) => p.includes("controller")) || Object.keys(srcMap)[0];
  if (first) openFile(first);
}

document.addEventListener("keydown", (e) => {
  const mod = e.metaKey || e.ctrlKey;
  if (mod && e.key === "p" && !e.shiftKey) {
    e.preventDefault();
    showPicker(fileItems(), "file or class…");
  } else if (mod && e.shiftKey && (e.key === "r" || e.key === "R")) {
    e.preventDefault();
    showRelated();
  }
});
document.getElementById("btnOpen").onclick = () => showPicker(fileItems(), "file or class…");
document.getElementById("btnRelated").onclick = showRelated;
document.getElementById("btnGaps").onclick = () => {
  if (!analysis) return;
  showPicker(
    analysis.gaps.map((g) => ({
      label: `${g.file ? g.file.split("/").slice(-2).join("/") : "(app)"} — ${g.message.slice(0, 90)}`,
      search: `${g.file} ${g.message}`,
      run: () => { if (g.file in srcMap) openFile(g.file); },
    })),
    `${analysis.gaps.length} ingest gaps (the coverage ledger)`,
  );
};

// Test hook: the verify script (and curious consoles) drive the page
// through this, editor-widget agnostic.
window.__ide = {
  rpc,
  openFile,
  get analysis() { return analysis; },
  get activePath() { return activePath; },
  get srcMap() { return srcMap; },
  reanalyze,
};

boot();
