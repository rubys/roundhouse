// Browser entry point for the Phase 0 spike. Loads roundhouse_wasm.wasm and
// the real-blog fixture, runs the SAME shared driver the Node validator uses
// (transpile.mjs + wasi-shim.mjs), and renders the emitted files — with a
// target dropdown so the multi-target story is visible. No npm, no bundler,
// no WebContainer: three static .mjs/.wasm files served over plain HTTP.

import { loadCompiler } from "./transpile.mjs";

const els = {
  status: document.getElementById("status"),
  target: document.getElementById("target"),
  files: document.getElementById("files"),
  code: document.getElementById("code"),
};

const TARGETS = ["typescript", "go", "rust", "python", "elixir", "crystal"];
for (const t of TARGETS) {
  const opt = document.createElement("option");
  opt.value = t;
  opt.textContent = t;
  els.target.appendChild(opt);
}

let compiler = null;
let srcMap = null;
let lastResult = null;

function setStatus(msg, kind = "") {
  els.status.textContent = msg;
  els.status.className = kind;
}

function renderResult(result, ms) {
  els.files.innerHTML = "";
  if (result.error) {
    setStatus(`error: ${result.error}`, "err");
    els.code.textContent = "";
    return;
  }
  setStatus(`${result.language}: ${result.files.length} files in ${ms.toFixed(1)} ms`, "ok");
  result.files.forEach((f, i) => {
    const li = document.createElement("li");
    const btn = document.createElement("button");
    btn.textContent = `${f.path}  (${f.content.length} B)`;
    btn.onclick = () => showFile(i);
    li.appendChild(btn);
    els.files.appendChild(li);
  });
  showFile(0);
}

function showFile(i) {
  lastResult && (els.code.textContent = lastResult.files[i].content);
  [...els.files.querySelectorAll("button")].forEach((b, j) =>
    b.classList.toggle("active", j === i));
}

function run() {
  const lang = els.target.value;
  const t0 = performance.now();
  lastResult = compiler.transpile(lang, srcMap);
  const t1 = performance.now();
  renderResult(lastResult, t1 - t0);
}

async function boot() {
  setStatus("loading wasm + fixture…");
  const tLoad0 = performance.now();
  const [wasmBytes, fixture] = await Promise.all([
    fetch("roundhouse_wasm.wasm").then((r) => r.arrayBuffer()),
    fetch("fixture.json").then((r) => r.json()),
  ]);
  srcMap = fixture;
  // Route wasm stdout/stderr to the console rather than the page.
  compiler = await loadCompiler(wasmBytes, {
    onStdout: (s) => console.log("[wasm]", s),
    onStderr: (s) => console.warn("[wasm]", s),
  });
  const tLoad1 = performance.now();
  setStatus(
    `compiler ready: ${(wasmBytes.byteLength / 1024 / 1024).toFixed(2)} MB wasm + ` +
    `${Object.keys(srcMap).length}-file fixture loaded in ${(tLoad1 - tLoad0).toFixed(1)} ms`);
  els.target.onchange = run;
  run();
}

boot().catch((e) => setStatus(`boot failed: ${e.message}`, "err"));
