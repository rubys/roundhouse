// Shared compiler driver: wraps the roundhouse-wasm C-ABI
// (rh_alloc / transpile / rh_dealloc, packed u64 return) in a clean
// transpile(language, srcMap) -> { files | error } interface. Uses only
// WebAssembly + TextEncoder/Decoder, so the same module drives both the
// Node validator and the browser page.
//
// Lives in wasm/lib/ alongside the compiler binary (roundhouse_wasm.wasm) and
// the seed app (fixture.json), shared by both /playground/ and /studio/.
// loadDefaultCompiler / loadFixture resolve those two assets relative to THIS
// module via import.meta.url, so a surface page needn't know where lib/ sits.

import { makeWasi } from "./wasi-shim.mjs";

// wasmBytes: ArrayBuffer | Uint8Array of roundhouse_wasm.wasm.
// Returns { transpile(language, srcMap) } where srcMap is { path: content }.
export async function loadCompiler(wasmBytes, wasiOpts = {}) {
  const memoryRef = { value: null };
  const wasi = makeWasi(memoryRef, wasiOpts);
  const { instance } = await WebAssembly.instantiate(wasmBytes, {
    wasi_snapshot_preview1: wasi,
  });
  memoryRef.value = instance.exports.memory;
  if (instance.exports._initialize) instance.exports._initialize();

  const { rh_alloc, rh_dealloc, transpile, memory } = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  return {
    // opts.profile (typescript only): "worker" | "node-async" | "node-sync".
    // Omitted ⇒ the default emit (what /playground/ shows). /studio/ passes
    // "worker" to get the SharedWorker browser app it runs.
    transpile(language, srcMap, { profile } = {}) {
      const input = JSON.stringify(profile ? { language, src: srcMap, profile } : { language, src: srcMap });
      const inputBytes = encoder.encode(input);

      const inputPtr = rh_alloc(inputBytes.length);
      new Uint8Array(memory.buffer, inputPtr, inputBytes.length).set(inputBytes);

      const packed = transpile(inputPtr, inputBytes.length);
      const outPtr = Number(packed & 0xffffffffn);
      const outLen = Number(packed >> 32n);

      const outBytes = new Uint8Array(memory.buffer, outPtr, outLen).slice();
      const output = decoder.decode(outBytes);

      rh_dealloc(inputPtr, inputBytes.length);
      rh_dealloc(outPtr, outLen);

      return JSON.parse(output);
    },
  };
}

// Browser convenience: fetch the lib's own roundhouse_wasm.wasm (resolved
// relative to this module, not the loading page) and instantiate it. Falls
// back through to loadCompiler so the byte-level path stays single-sourced.
export async function loadDefaultCompiler(wasiOpts = {}) {
  const url = new URL("./roundhouse_wasm.wasm", import.meta.url);
  const bytes = await fetch(url).then((r) => r.arrayBuffer());
  return loadCompiler(bytes, wasiOpts);
}

// Browser convenience: fetch the lib's seed app (the real-blog fixture as a
// { path: content } map), resolved relative to this module.
export async function loadFixture() {
  const url = new URL("./fixture.json", import.meta.url);
  return fetch(url).then((r) => r.json());
}
