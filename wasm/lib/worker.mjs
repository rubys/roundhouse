// Shared compiler Web Worker: owns one wasm engine instance so a multi-second
// pass (a Mastodon transpile, a whole-app analyze) never blocks the UI thread.
// Backs BOTH /playground/ and /ide/ through wasm-client.mjs.
//
// Protocol: the page posts {id, op, args}; the worker replies {id, result} or
// {id, error}. Wasm calls are synchronous and non-interruptible, so a hang
// (an emitter looping on a degraded app) can't be cancelled from in here — the
// main-thread client watches the clock and terminates + restarts this worker,
// which is why it holds no state the client can't rebuild.
import { loadEngine } from "./engine.mjs";

// RPC op → wasm export name. Every export is JSON-in / JSON-out.
const OP_EXPORT = {
  transpile: "transpile",
  analyze: "analyze_app",
  complete: "complete",
  typeAt: "type_at",
  related: "related_files",
  traceroute: "traceroute",
  traceTargets: "trace_targets",
};

// Assigned synchronously by the `init` handler so an op that arrives before
// instantiation finishes still awaits the same promise (message order is
// preserved, and init is always posted first).
let enginePromise = null;

self.onmessage = async (e) => {
  const { id, op, args } = e.data;
  try {
    if (op === "init") {
      enginePromise = fetch(args.wasmUrl)
        .then((r) => {
          if (!r.ok) throw new Error(`wasm fetch: ${r.status}`);
          return r.arrayBuffer();
        })
        .then((bytes) => loadEngine(bytes, { onStderr: (s) => console.warn("[wasm]", s) }));
      await enginePromise;
      self.postMessage({ id, result: true });
      return;
    }
    if (!enginePromise) throw new Error("worker not initialized");
    const engine = await enginePromise;
    const exportName = OP_EXPORT[op];
    if (!exportName) throw new Error(`unknown op: ${op}`);
    const t0 = performance.now();
    const result = engine.callExport(exportName, args);
    // The /ide/ status line reports analyze wall time.
    if (op === "analyze" && result && typeof result === "object" && !result.error) {
      result.elapsed_ms = Math.round(performance.now() - t0);
    }
    self.postMessage({ id, result });
  } catch (err) {
    self.postMessage({ id, error: String(err?.message || err) });
  }
};
