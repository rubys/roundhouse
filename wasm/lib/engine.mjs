// Unified wasm driver: one instance, one generic `callExport(name, obj)` over
// the roundhouse C-ABI (rh_alloc → export(ptr,len) → packed-u64 → rh_dealloc).
// It hosts EVERY entry point the compiler wasm exposes — transpile, analyze_app,
// complete, type_at, related_files, traceroute, trace_targets — so a single Web
// Worker can back both /playground/ (emit) and /ide/ (analyze). Web-platform
// APIs only, so the same module runs under Node for validation.
import { makeWasi } from "./wasi-shim.mjs";

export async function loadEngine(wasmBytes, wasiOpts = {}) {
  const memoryRef = { value: null };
  const { instance } = await WebAssembly.instantiate(wasmBytes, {
    wasi_snapshot_preview1: makeWasi(memoryRef, wasiOpts),
  });
  memoryRef.value = instance.exports.memory;
  if (instance.exports._initialize) instance.exports._initialize();

  const ex = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  return {
    // Call any JSON-in/JSON-out export by name. Every compiler entry point
    // shares this shape (a single JSON arg, a packed (ptr,len) return).
    callExport(name, obj) {
      const fn = ex[name];
      if (typeof fn !== "function") throw new Error(`no such wasm export: ${name}`);
      const bytes = encoder.encode(JSON.stringify(obj ?? {}));
      const ptr = ex.rh_alloc(bytes.length);
      new Uint8Array(ex.memory.buffer, ptr, bytes.length).set(bytes);
      const packed = fn(ptr, bytes.length);
      const outPtr = Number(packed & 0xffffffffn);
      const outLen = Number(packed >> 32n);
      const out = decoder.decode(new Uint8Array(ex.memory.buffer, outPtr, outLen).slice());
      ex.rh_dealloc(ptr, bytes.length);
      ex.rh_dealloc(outPtr, outLen);
      return JSON.parse(out);
    },
  };
}
