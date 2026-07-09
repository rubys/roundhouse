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
//   ⌘⇧T          → traceroute: pick a route/action, pin its request
//                  chain in the right panel (hops jump; footer is the
//                  priced gap report with copyable candidate RBS)
//
// Edits re-analyze in the worker (debounced); queries answer from the
// previous snapshot meanwhile — stale-by-one-edit, the standard
// fast-language-server trade.

import { loadMonaco, registerTypedCompletion } from "../lib/editor.js";
import { buildTree, allDirPaths, renderTree } from "../lib/tree.js";
import { createClient } from "../lib/wasm-client.mjs";

const els = {
  status: document.getElementById("status"),
  counts: document.getElementById("counts"),
  app: document.getElementById("app"),
  appLabel: document.getElementById("appLabel"),
  tree: document.getElementById("tree"),
  editor: document.getElementById("editor"),
  editorHead: document.getElementById("editorHead"),
  picker: document.getElementById("picker"),
  pickerInput: document.getElementById("pickerInput"),
  pickerList: document.getElementById("pickerList"),
};

// ── Worker RPC ───────────────────────────────────────────────────────
// The analyzer wasm runs in the shared compiler worker (lib/worker.mjs) behind
// a watchdog client, so a whole-app pass never blocks the UI and a hang/trap
// restarts the worker instead of freezing the tab. `rpc` is the thin op bridge
// the rest of this file speaks.
const client = createClient({
  workerUrl: new URL("../lib/worker.mjs", import.meta.url),
  wasmUrl: new URL("../lib/roundhouse_wasm.wasm", import.meta.url).href,
  timeoutMs: 30000,
  onRestart: (reason) => status(`compiler restarted — ${reason}`),
});
const rpc = (op, args) => client.call(op, args);

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
let apps = [];               // app-picker manifest entries ({name,label,src})

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
  syncTraceToCursor();
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

// ── Trace panel (#63) ────────────────────────────────────────────────
// One `traceroute` result, pinned in the right column until replaced
// or closed. Filter hops group into contiguous runs by defining
// class/concern (the chain is built that way, so grouping preserves
// execution order); runs collapse by default with their typed assigns
// floated to the header, and force badges when a hop inside is
// skipped or unresolved. Every row jumps; the cursor highlights the
// hop it is inside. The footer is the gap report: the completeness
// claim, or priced entries split user-actionable vs tool-coverage,
// with a copy button on pre-filled candidate RBS.

const traceEls = {
  panel: document.getElementById("trace"),
  route: document.getElementById("traceRoute"),
  hops: document.getElementById("traceHops"),
  foot: document.getElementById("traceFoot"),
};
let trace = null;
let hopRows = [];              // [{el, file, line}] for cursor sync
const openGroups = new Set();  // user-toggled group keys

function snakePath(cls) {
  return cls.replace(/::/g, "/").replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
}

function goTo(path, line) {
  if (!(path in srcMap)) return;
  openFile(path);
  if (line) {
    editor.revealLineInCenter(line);
    editor.setPosition({ lineNumber: line, column: 1 });
  }
}

async function runTrace(query) {
  const result = await rpc("traceroute", { query });
  if (result.error) { status(result.error); return; }
  trace = result;
  openGroups.clear();
  renderTrace();
}

function closeTrace() {
  trace = null;
  hopRows = [];
  traceEls.panel.classList.remove("open");
}

function span(cls, text) {
  const el = document.createElement("span");
  el.className = cls;
  el.textContent = text;
  return el;
}

function hopRow(hop) {
  const el = document.createElement("button");
  el.className = "thop";
  if (hop.applies === false) el.classList.add("off");
  if (hop.applies !== false && hop.resolved === false) el.classList.add("unresolved");
  el.appendChild(span("kind", hop.filter_kind || hop.kind));
  el.appendChild(span("name", hop.name || hop.detail || ""));
  if (hop.condition) el.appendChild(span("meta", hop.condition));
  if (hop.skipped_by) el.appendChild(span("meta", `skipped by ${hop.skipped_by}`));
  else if (hop.applies === false) {
    const gate = hop.only?.length ? `only: ${hop.only.join(", ")}` : hop.except?.length ? `except: ${hop.except.join(", ")}` : "";
    if (gate) el.appendChild(span("meta", gate));
  }
  for (const [k, v] of Object.entries(hop.assigns || {})) {
    el.appendChild(span("assign", `${k} : ${v}`));
  }
  // #63 phase 5: N+1 findings on the hop containing the access site.
  // The badge jumps to the read (which may be a partial, not the hop's
  // own file); the tooltip carries the full finding with the fix.
  for (const f of hop.n_plus_one || []) {
    const b = span("nplus", `N+1 :${f.association}`);
    b.title = f.message;
    if (f.file) {
      b.onclick = (e) => { e.stopPropagation(); goTo(f.file, f.line || null); };
    }
    el.appendChild(b);
  }
  if ((hop.effects || []).some((e) => e.startsWith("Db"))) {
    const fx = span("fx", "⛁");
    fx.title = (hop.effects || []).join(", ");
    el.appendChild(fx);
  }
  if (hop.formats?.length) el.appendChild(span("meta", hop.formats.join(" · ")));
  if (hop.file) {
    el.appendChild(span("src", hop.file.split("/").slice(-1)[0] + (hop.line ? `:${hop.line}` : "")));
    el.onclick = () => goTo(hop.file, hop.line);
  }
  hopRows.push({ el, file: hop.file || null, line: hop.line || null });
  return el;
}

function renderTrace() {
  if (!trace) return;
  traceEls.panel.classList.add("open");
  traceEls.route.textContent = trace.route;
  traceEls.hops.innerHTML = "";
  traceEls.foot.innerHTML = "";
  hopRows = [];

  // Segment filter hops into contiguous defined_in runs; everything
  // else renders flat, in chain order.
  const items = [];
  for (const hop of trace.hops) {
    const last = items[items.length - 1];
    if (hop.kind === "filter" && last?.group && last.defined_in === hop.defined_in) {
      last.hops.push(hop);
    } else if (hop.kind === "filter") {
      items.push({ group: true, defined_in: hop.defined_in, via: hop.included_via, hops: [hop] });
    } else {
      items.push({ group: false, hop });
    }
  }

  items.forEach((item, gi) => {
    if (!item.group) {
      const hop = item.hop;
      traceEls.hops.appendChild(hopRow(hop));
      // Partials nest under the view hop; files resolve by convention
      // against the source tree (partial names carry no file).
      for (const p of hop.partials || []) {
        const row = document.createElement("button");
        row.className = "thop tsub";
        row.appendChild(span("kind", "partial"));
        row.appendChild(span("name", p));
        const base = `app/views/${p}.`;
        const file = Object.keys(srcMap).find((f) => f.startsWith(base));
        if (file) row.onclick = () => goTo(file, null);
        hopRows.push({ el: row, file: file || null, line: null });
        traceEls.hops.appendChild(row);
      }
      return;
    }
    const off = item.hops.filter((h) => h.applies === false).length;
    const soft = item.hops.some((h) => h.applies !== false && h.resolved === false);
    const skipped = item.hops.some((h) => h.skipped_by);
    const nplus = item.hops.some((h) => (h.n_plus_one || []).length);
    // Own-controller runs and anything demanding attention start open.
    const forced = soft || skipped || nplus;
    const key = `${gi}:${item.defined_in}`;
    const expanded = forced || openGroups.has(key) || item.defined_in === trace.controller;

    const head = document.createElement("button");
    head.className = "tgroup";
    head.appendChild(span("tw", expanded ? "▾" : "▸"));
    head.appendChild(span("name", item.defined_in));
    head.appendChild(span("count", `· ${item.hops.length} filter${item.hops.length > 1 ? "s" : ""}`));
    if (item.via && item.via !== item.defined_in) head.appendChild(span("src", `via ${item.via}`));
    // Float the applied hops' typed assigns up so collapse hides nothing typed.
    const seen = new Set();
    for (const h of item.hops) {
      if (h.applies === false) continue;
      for (const [k, v] of Object.entries(h.assigns || {})) {
        if (seen.has(k)) continue;
        seen.add(k);
        head.appendChild(span("assign", `${k} : ${v}`));
      }
    }
    if (off) head.appendChild(span("badge", `${off} don't run`));
    if (soft) head.appendChild(span("badge", "⋯ unresolved"));
    if (nplus) head.appendChild(span("nplus", "N+1"));
    head.onclick = () => {
      openGroups.has(key) ? openGroups.delete(key) : openGroups.add(key);
      renderTrace();
    };
    traceEls.hops.appendChild(head);
    if (expanded) {
      for (const h of item.hops) {
        const row = hopRow(h);
        row.classList.add("tsub");
        traceEls.hops.appendChild(row);
      }
    }
  });

  renderTraceFooter();
  syncTraceToCursor();
}

function sidecarFor(boundary, defLine) {
  const cls = boundary.split("#")[0];
  const segs = cls.split("::");
  const lines = [];
  segs.forEach((seg, i) =>
    lines.push("  ".repeat(i) + (i === segs.length - 1 ? `class ${seg}` : `module ${seg}`)));
  lines.push("  ".repeat(segs.length) + defLine);
  for (let i = segs.length - 1; i >= 0; i--) lines.push("  ".repeat(i) + "end");
  return lines.join("\n") + "\n";
}

function renderTraceFooter() {
  const cov = trace.coverage || {};
  if (cov.complete) {
    const div = document.createElement("div");
    div.className = "complete";
    div.textContent = `✓ trace complete — all ${cov.total_hops} hops resolved`;
    traceEls.foot.appendChild(div);
    return;
  }
  const sum = document.createElement("div");
  sum.className = "summary";
  sum.textContent =
    `── ${trace.gaps.length} gap(s) · ${cov.resolved_hops}/${cov.total_hops} hops resolved ──`;
  traceEls.foot.appendChild(sum);
  for (const g of trace.gaps) {
    const div = document.createElement("div");
    div.className = "gap" + (g.kind === "ingest_gap" ? " ours" : "");
    div.appendChild(span("tag", g.kind === "ingest_gap" ? "[tool]" : "[boundary]"));
    div.appendChild(span("", `[${g.blocked_hops} hop${g.blocked_hops > 1 ? "s" : ""}] ${g.detail}`));
    if (g.candidate_rbs) {
      const pre = document.createElement("pre");
      pre.textContent = g.candidate_rbs;
      div.appendChild(pre);
      const btn = document.createElement("button");
      btn.textContent = "copy RBS";
      btn.onclick = async () => {
        const cls = g.boundary.split("#")[0];
        await navigator.clipboard.writeText(sidecarFor(g.boundary, g.candidate_rbs));
        status(`candidate signature copied — save as sig/${snakePath(cls)}.rbs`);
      };
      div.appendChild(btn);
    }
    traceEls.foot.appendChild(div);
  }
}

function syncTraceToCursor() {
  if (!trace || !activePath) return;
  const cursor = editor?.getPosition()?.lineNumber ?? 0;
  let best = null;
  for (const r of hopRows) {
    r.el.classList.remove("current");
    if (r.file !== activePath) continue;
    const d = r.line == null ? 1e9 : Math.abs(r.line - cursor);
    if (!best || d < best.d) best = { r, d };
  }
  if (best) {
    best.r.el.classList.add("current");
    best.r.el.scrollIntoView({ block: "nearest" });
  }
}

async function showTracePicker() {
  const targets = await rpc("traceTargets", {});
  if (targets.error) { status(targets.error); return; }
  // Context: when a controller file is active, its actions float first.
  const active = activePath?.match(/app\/controllers\/(.+)\.rb$/)?.[1];
  const ordered = active
    ? [...targets.filter((t) => snakePath(t.controller) === active),
       ...targets.filter((t) => snakePath(t.controller) !== active)]
    : targets;
  // The list renders 40 rows at a time (type to filter) — say how
  // many targets sit behind the window so the cut is never silent.
  showPicker(
    ordered.map((t) => ({
      label: t.label,
      search: `${t.label} ${t.query}`,
      run: () => runTrace(t.query),
    })),
    `trace a route or Controller#action… (${targets.length} targets)`,
  );
}

// ── Boot ─────────────────────────────────────────────────────────────
// ── App bundles ──────────────────────────────────────────────────────
// Each app is a source-only bundle from bundle-src.mjs. analyze_app is
// stateless (it re-ingests the whole tree every call), so switching apps
// is just: swap srcMap, drop the outgoing app's editor state, reanalyze.
async function loadBundle(url) {
  const resp = await fetch(url);
  if (!resp.ok) throw new Error(`${resp.status}`);
  return resp.json();
}

// Keep ?app= in the address bar in step with the picker, so the URL is a
// shareable deep-link (ide/?app=mastodon) — updated in place, no reload.
function syncAppUrl(name) {
  const u = new URL(location.href);
  u.searchParams.set("app", name);
  history.replaceState(null, "", u);
}

async function loadApp(entry) {
  status(`loading ${entry.label || entry.name}…`);
  let bundle;
  try {
    bundle = await loadBundle(new URL(`./${entry.src}`, import.meta.url));
  } catch (e) {
    status(`could not load ${entry.src} (${e.message}) — generate it with bundle-src.mjs`);
    return;
  }
  // Tear down the outgoing app: MRU, pinned trace, and the stale analysis
  // snapshot all belong to the app we're leaving. The outgoing models stay
  // attached until the new file's model replaces the active one — disposing
  // the editor's live model out from under it upsets Monaco's listeners.
  closeTrace();
  hidePicker();
  const stale = [...models.values()];
  models.clear();
  modelPath.clear();
  mru.length = 0;
  activePath = null;
  analysis = null;
  els.counts.textContent = "";

  srcMap = bundle.src;
  document.title = `roundhouse ide — ${bundle.name || "rails app"}`;
  redrawTree();
  await reanalyze();
  const first = bundle.open || Object.keys(srcMap).find((p) => p.includes("controller")) || Object.keys(srcMap)[0];
  if (first) openFile(first);
  for (const m of stale) m.dispose(); // safe now: the editor holds a fresh model
}

async function boot() {
  status("loading editor…");
  monaco = await loadMonaco();

  editor = monaco.editor.create(els.editor, {
    value: "", language: "ruby", automaticLayout: true,
    minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false,
    tabSize: 2,
  });
  // The pinned trace tracks the cursor: whichever hop the position is
  // inside (same file, nearest line) highlights as "you are here".
  editor.onDidChangeCursorPosition(() => syncTraceToCursor());

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

  // Completion: the shared provider (lib/editor.js) over the worker's
  // last-good snapshot; the model resolves which file is being completed.
  registerTypedCompletion(monaco, ["ruby", "html"], async (text, line, character, model) => {
    const path = modelPath.get(model);
    if (!path) return [];
    return rpc("complete", { path, text, line, character });
  });

  status("loading analyzer…");
  await client.ready();

  // App picker. apps.json lists the shipped Rails app bundles (each `src`
  // is a bundle-src.mjs output). Absent — plain local dev with a single
  // hand-bundled app-src.json — falls back to that lone Mastodon bundle
  // with the selector hidden, preserving the pre-picker behavior (and the
  // verify-ide harness, which drives that default).
  let manifest = null;
  try { manifest = await loadBundle(new URL("./apps.json", import.meta.url)); } catch { /* single-app dev */ }
  if (manifest && manifest.apps && manifest.apps.length) {
    apps = manifest.apps;
    for (const a of apps) {
      const opt = document.createElement("option");
      opt.value = a.name;
      opt.textContent = a.label || a.name;
      els.app.appendChild(opt);
    }
    // Deep-link: ?app=<name> wins over the manifest default (falls back to it,
    // then to the first app, if the param is absent or unknown).
    const want = new URLSearchParams(location.search).get("app");
    const def = (want && apps.find((a) => a.name === want))
      || (manifest.default && apps.find((a) => a.name === manifest.default))
      || apps[0];
    els.app.value = def.name;
    els.app.onchange = () => {
      const a = apps.find((x) => x.name === els.app.value);
      if (a) { syncAppUrl(a.name); loadApp(a); }
    };
    await loadApp(def);
  } else {
    els.appLabel.style.display = els.app.style.display = "none";
    await loadApp({ name: "app", src: "app-src.json" });
  }
}

document.addEventListener("keydown", (e) => {
  const mod = e.metaKey || e.ctrlKey;
  if (mod && e.key === "p" && !e.shiftKey) {
    e.preventDefault();
    showPicker(fileItems(), "file or class…");
  } else if (mod && e.shiftKey && (e.key === "r" || e.key === "R")) {
    e.preventDefault();
    showRelated();
  } else if (mod && e.shiftKey && (e.key === "t" || e.key === "T")) {
    e.preventDefault();
    showTracePicker();
  }
});
document.getElementById("btnOpen").onclick = () => showPicker(fileItems(), "file or class…");
document.getElementById("btnRelated").onclick = showRelated;
document.getElementById("btnTrace").onclick = showTracePicker;
document.getElementById("traceClose").onclick = closeTrace;
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
  runTrace,
  loadApp,
  get analysis() { return analysis; },
  get activePath() { return activePath; },
  get srcMap() { return srcMap; },
  get trace() { return trace; },
  get apps() { return apps; },
  reanalyze,
};

boot();
