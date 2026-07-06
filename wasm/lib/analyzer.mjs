// Shared analysis driver: wraps the roundhouse-wasm query C-ABI
// (analyze_app / complete / type_at / related_files — same
// rh_alloc/packed-u64 protocol as transpile.mjs) in a clean object
// interface. Uses only web-platform APIs, so the same module drives
// Node validation and the browser /ide/ worker.
//
// The wasm side keeps a thread-local **last-good snapshot**: analyze()
// refreshes it; every query answers from it. Queries before the first
// analyze() return {error}.

import { makeWasi } from "./wasi-shim.mjs";

// wasmBytes: ArrayBuffer | Uint8Array of roundhouse_wasm.wasm.
export async function loadAnalyzer(wasmBytes, wasiOpts = {}) {
  const memoryRef = { value: null };
  const { instance } = await WebAssembly.instantiate(wasmBytes, {
    wasi_snapshot_preview1: makeWasi(memoryRef, wasiOpts),
  });
  memoryRef.value = instance.exports.memory;
  if (instance.exports._initialize) instance.exports._initialize();

  const ex = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  function call(name, obj) {
    const bytes = encoder.encode(JSON.stringify(obj));
    const ptr = ex.rh_alloc(bytes.length);
    new Uint8Array(ex.memory.buffer, ptr, bytes.length).set(bytes);
    const packed = ex[name](ptr, bytes.length);
    const outPtr = Number(packed & 0xffffffffn);
    const outLen = Number(packed >> 32n);
    const out = decoder.decode(new Uint8Array(ex.memory.buffer, outPtr, outLen).slice());
    ex.rh_dealloc(ptr, bytes.length);
    ex.rh_dealloc(outPtr, outLen);
    return JSON.parse(out);
  }

  return {
    // srcMap: { "app/models/x.rb": "...", ... }. Returns AnalyzeOutput:
    // { diagnostics, gaps, files, classes } (or { error }).
    analyze(srcMap) {
      return call("analyze_app", { src: srcMap });
    },
    // text is the CURRENT buffer (may be ahead of the snapshot);
    // line/character are 0-based, UTF-16 (Monaco/LSP convention).
    complete(path, text, line, character) {
      return call("complete", { path, text, line, character });
    },
    // Positions refer to the snapshot's own analyzed text.
    typeAt(path, line, character) {
      return call("type_at", { path, line, character });
    },
    relatedFiles(path) {
      return call("related_files", { path });
    },
    // "Controller#action" or "[VERB ]/path" → the full request chain
    // (hops + coverage + gap footer), the same JSON the MCP tool
    // returns. {error} when the query names nothing known.
    traceroute(query) {
      return call("traceroute", { query });
    },
    // Everything traceroute can trace (routes + unrouted
    // view-rendering actions), for pickers.
    traceTargets() {
      return call("trace_targets", {});
    },
  };
}

// Browser convenience: fetch lib/'s own wasm relative to this module.
export async function loadDefaultAnalyzer(wasiOpts = {}) {
  const url = new URL("./roundhouse_wasm.wasm", import.meta.url);
  const bytes = await fetch(url).then((r) => r.arrayBuffer());
  return loadAnalyzer(bytes, wasiOpts);
}
