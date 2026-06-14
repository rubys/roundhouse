// Shared compiler driver: wraps the roundhouse-wasm C-ABI
// (rh_alloc / transpile / rh_dealloc, packed u64 return) in a clean
// transpile(language, srcMap) -> { files | error } interface. Uses only
// WebAssembly + TextEncoder/Decoder, so the same module drives both the
// Node validator and the browser page.

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
    transpile(language, srcMap) {
      const input = JSON.stringify({ language, src: srcMap });
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
