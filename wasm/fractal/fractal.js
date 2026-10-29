// Fractal pass explorer: edit a small Rails app, pick the file to compile
// (`--file`), and step through Fractal's passes in order. Each stage is the
// compiled file after one more pass: what the pass rewrote, and what it
// looked at and left alone, with why.
//
// Everything runs in the browser: the same roundhouse wasm the playground
// uses, through its `fractal_stages` export (see wasm/src/lib.rs and
// src/emit/fractal/stages.rs). Serve wasm/ (or the published site) as the
// web root so ../lib/ resolves:
//   python3 -m http.server 8099   # from wasm/
//   open http://localhost:8099/fractal/

import { createClient } from "../lib/wasm-client.mjs";

// Small apps to compile, one per registered pass. In the Article model,
// `inline-delegate` writes the simple `delegate` calls out as the methods
// Active Support would generate, and declines the one with `prefix:`. In
// the Event model, `lower-duration` writes each literal Duration
// chain as the one Time call its Duration would make, and declines a
// constant's value, a non-literal receiver and a time passed to `ago`.
// Nothing here is from a real application.
const LOCKFILE = `GEM
  remote: https://rubygems.org/
  specs:
    activerecord (8.2.0.alpha)
      activemodel (= 8.2.0.alpha)
      activesupport (= 8.2.0.alpha)
    activesupport (8.2.0.alpha)

PLATFORMS
  ruby

DEPENDENCIES
  activerecord

BUNDLED WITH
   2.6.9
`;

const APPLICATION_RECORD = `class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
end
`;

const EXAMPLES = {
  article: {
    label: "Article model",
    file: "app/models/article.rb",
    src: {
      "app/models/article.rb": `# frozen_string_literal: true

class Article < ApplicationRecord
  belongs_to :author
  enum :status, { draft: 0, published: 1, archived: 2 }

  delegate :name, :email, to: :author
  delegate :time_zone, to: :author, prefix: true

  def listed?
    published? && !archived?
  end

  def byline = "by #{name}"
end
`,
      "app/models/application_record.rb": APPLICATION_RECORD,
      // Passes read gem versions from here: inline-delegate reproduces the
      // source this Active Support generates.
      "Gemfile.lock": LOCKFILE,
    },
  },
  event: {
    label: "Event model",
    file: "app/models/event.rb",
    src: {
      "app/models/event.rb": `# frozen_string_literal: true

class Event < ApplicationRecord
  LISTED_UNTIL = 14.days.from_now

  scope :past, -> { where(ends_at: ...7.days.ago) }

  def starting_soon?
    starts_at < 3.days.from_now
  end

  def recently_edited?
    edited_at.present? && edited_at > 2.hours.ago
  end

  def registration_closes = 1.fortnight.after

  def announced_at = notice_days.days.ago

  def reminder_at = 30.minutes.ago(starts_at)
end
`,
      "app/models/application_record.rb": APPLICATION_RECORD,
      // lower-duration reproduces what this Active Support's Duration calls.
      "Gemfile.lock": LOCKFILE,
    },
  },
};

const DEBOUNCE_MS = 350;

const $ = (id) => document.getElementById(id);
const els = {
  example: $("example"), file: $("file"), status: $("status"), pipeline: $("pipeline"),
  srcTabs: $("srcTabs"), editor: $("editor"), outTabs: $("outTabs"), outTitle: $("outTitle"),
  code: $("code"), notes: $("notes"), diff: $("diffToggle"), prev: $("prev"), next: $("next"),
};

let client = null;
let exampleKey = "article";
let src = {};           // the live, editable source tree
let srcPath = null;     // the file in the editor
let file = null;        // the file compiled (--file)
let result = null;      // last fractal_stages answer
let stage = null;       // selected stage index
let outPath = null;     // the file shown on the right
let seq = 0;
let timer = null;

function setStatus(text, cls = "") {
  els.status.textContent = text;
  els.status.className = cls;
}

function esc(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
}

function readHash() {
  const h = new URLSearchParams(location.hash.slice(1));
  return { example: h.get("example"), stage: h.get("stage") };
}

function writeHash() {
  const name = result?.stages?.[stage]?.pass;
  const h = new URLSearchParams({ example: exampleKey });
  if (name) h.set("stage", name);
  history.replaceState(null, "", `#${h}`);
}

function loadExample(key) {
  exampleKey = EXAMPLES[key] ? key : "article";
  const ex = EXAMPLES[exampleKey];
  src = { ...ex.src };
  srcPath = ex.file;
  file = ex.file;
  outPath = ex.file;
  els.example.value = exampleKey;
  renderSrcTabs();
  els.editor.value = src[srcPath];
}

function short(path) {
  return path.replace(/^app\/models\//, "");
}

function renderSrcTabs() {
  els.srcTabs.innerHTML = "";
  for (const path of Object.keys(src)) {
    const b = document.createElement("button");
    b.className = "tab" + (path === srcPath ? " active" : "") + (src[path] !== EXAMPLES[exampleKey].src[path] ? " dirty" : "");
    b.textContent = short(path);
    b.title = path;
    b.onclick = () => {
      srcPath = path;
      els.editor.value = src[path];
      renderSrcTabs();
    };
    els.srcTabs.appendChild(b);
  }
}

async function run() {
  const mine = ++seq;
  setStatus("running passes…");
  let res;
  try {
    res = await client.fractalStages(src, file ? [file] : []);
  } catch (e) {
    if (mine === seq) setStatus(String(e.message || e), "err");
    return;
  }
  if (mine !== seq) return;
  if (res.error) {
    setStatus(res.error, "err");
    return;
  }
  const previous = result?.stages?.[stage]?.pass;
  result = res;
  renderFiles();
  const names = result.stages.map((s) => s.pass);
  const wanted = previous ?? readHash().stage;
  stage = names.includes(wanted) ? names.indexOf(wanted) : names.length - 1;
  const rewrites = result.stages.reduce((n, s) => n + s.rewrites.length, 0);
  setStatus(`${rewrites} rewrite(s)`, "ok");
  render();
}

function schedule() {
  clearTimeout(timer);
  timer = setTimeout(run, DEBOUNCE_MS);
}

function renderFiles() {
  const list = result.candidates || [];
  els.file.innerHTML = list.map((p) => `<option value="${esc(p)}"${p === file ? " selected" : ""}>${esc(short(p))}</option>`).join("");
  if (!list.includes(file) && list.length) {
    file = list[0];
    schedule();
  }
}

function renderPipeline() {
  els.pipeline.innerHTML = "";
  result.stages.forEach((s, i) => {
    if (i > 0) {
      const a = document.createElement("span");
      a.className = "arrow";
      a.textContent = "→";
      els.pipeline.appendChild(a);
    }
    const b = document.createElement("button");
    const changed = s.rewrites.length > 0;
    b.className = "chip" + (i === stage ? " active" : "") + (i > 0 ? (changed ? " changed" : " idle") : "");
    const count = i === 0 ? "input" : changed ? `${s.rewrites.length} rewrite(s)` : s.declined.length ? `no change · ${s.declined.length} declined` : "no change";
    b.innerHTML = `<div class="n">${i === 0 ? "stage 0" : `pass ${i}`}</div><div class="name">${esc(s.pass)}</div><div class="count">${esc(count)}</div>`;
    b.title = s.summary;
    b.onclick = () => select(i);
    els.pipeline.appendChild(b);
  });
}

function select(i) {
  stage = Math.max(0, Math.min(result.stages.length - 1, i));
  render();
}

// Longest-common-subsequence line diff; the files here are small.
function diffLines(a, b) {
  const n = a.length, m = b.length;
  const dp = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const out = [];
  let i = 0, j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) { out.push({ k: " ", t: b[j], no: j + 1 }); i++; j++; }
    else if (j < m && (i >= n || dp[i][j + 1] > dp[i + 1][j])) { out.push({ k: "+", t: b[j], no: j + 1 }); j++; }
    else { out.push({ k: "-", t: a[i], no: "" }); i++; }
  }
  return out;
}

function renderCode() {
  const s = result.stages[stage];
  const files = s.files.map((f) => f.path);
  if (!files.includes(outPath)) outPath = files[0];
  els.outTabs.innerHTML = "";
  for (const path of files) {
    const b = document.createElement("button");
    b.className = "tab" + (path === outPath ? " active" : "");
    b.textContent = short(path);
    b.title = path;
    b.onclick = () => { outPath = path; renderCode(); };
    els.outTabs.appendChild(b);
  }
  els.outTitle.textContent = stage === 0 ? "source (stage 0)" : `after pass ${stage}: ${s.pass}`;
  const now = (s.files.find((f) => f.path === outPath)?.content ?? "").replace(/\n$/, "").split("\n");
  let rows;
  if (els.diff.checked && stage > 0) {
    const before = (result.stages[stage - 1].files.find((f) => f.path === outPath)?.content ?? "").replace(/\n$/, "").split("\n");
    rows = diffLines(before, now);
  } else {
    rows = now.map((t, i) => ({ k: " ", t, no: i + 1 }));
  }
  els.code.innerHTML = rows
    .map((r) => `<div class="line${r.k === "+" ? " add" : r.k === "-" ? " del" : ""}"><span class="no">${r.no}</span><span class="mark">${r.k === " " ? "" : r.k}</span><span class="txt">${esc(r.t) || " "}</span></div>`)
    .join("");
}

// Escape `text`, setting its `backticked` spans as code.
function code(text) {
  return esc(text).replace(/`([^`]+)`/g, "<code>$1</code>");
}

// `app/models/order.rb:8:5` -> `order.rb:8:5`
function at(where) {
  return short(where);
}

function renderNotes() {
  const s = result.stages[stage];
  let html = `<div class="summary">${code(s.summary)}</div>`;
  if (stage > 0) {
    html += `<h3>Rewritten</h3>`;
    html += s.rewrites.length
      ? `<ul>${s.rewrites.map((r) => `<li><span class="key">${esc(at(r.at))}</span> <code>${esc(r.from)}</code><div class="to">→ <code>${esc(r.to)}</code></div></li>`).join("")}</ul>`
      : `<ul><li class="gap">Nothing in the compiled file for this pass.</li></ul>`;
    if (s.declined.length) {
      html += `<h3>Looked at and left alone</h3><ul>${s.declined.map((d) => `<li class="declined"><code>${esc(at(d.at))}</code> ${code(d.reason)}</li>`).join("")}</ul>`;
    }
  }
  if (result.skipped?.length) {
    html += `<h3>Not compiled</h3><ul>${result.skipped.map((k) => `<li class="declined"><code>${esc(k.file)}</code> ${code(k.reason)}</li>`).join("")}</ul>`;
  }
  if (result.gaps?.length) {
    html += `<h3>Ingest gaps (their files keep the app's code)</h3><ul>${result.gaps.map((g) => `<li class="gap">${esc(g)}</li>`).join("")}</ul>`;
  }
  els.notes.innerHTML = html;
}

function render() {
  if (!result?.stages?.length) {
    els.code.innerHTML = "";
    els.notes.innerHTML = `<div class="summary">Pick a file to compile to see Fractal's passes.</div>`;
    return;
  }
  renderPipeline();
  renderCode();
  renderNotes();
  els.prev.disabled = stage === 0;
  els.next.disabled = stage === result.stages.length - 1;
  writeHash();
}

function wire() {
  els.example.hidden = Object.keys(EXAMPLES).length < 2;
  els.example.innerHTML = Object.entries(EXAMPLES).map(([k, e]) => `<option value="${k}">${esc(e.label)}</option>`).join("");
  els.example.onchange = () => { loadExample(els.example.value); result = null; stage = null; schedule(); };
  els.file.onchange = () => { file = els.file.value; outPath = file; schedule(); };
  els.editor.addEventListener("input", () => { src[srcPath] = els.editor.value; renderSrcTabs(); schedule(); });
  els.diff.onchange = () => renderCode();
  els.prev.onclick = () => select(stage - 1);
  els.next.onclick = () => select(stage + 1);
  document.addEventListener("keydown", (e) => {
    if (e.target === els.editor || !result) return;
    if (e.key === "ArrowLeft") select(stage - 1);
    if (e.key === "ArrowRight") select(stage + 1);
  });
}

async function boot() {
  wire();
  loadExample(readHash().example || "article");
  client = createClient({
    workerUrl: new URL("../lib/worker.mjs", import.meta.url),
    wasmUrl: new URL("../lib/roundhouse_wasm.wasm", import.meta.url).href,
    onRestart: (why) => setStatus(`compiler restarted: ${why}`, "err"),
  });
  try {
    await client.ready();
  } catch (e) {
    setStatus(`could not load the compiler: ${e.message || e}`, "err");
    return;
  }
  await run();
}

boot();
