// The /ide/ analysis worker: owns the wasm analyzer instance so the
// 2-3s whole-app pass never blocks the UI thread. Protocol: the page
// posts {id, op, args}; the worker replies {id, result} (or
// {id, error}). Wasm calls are synchronous and non-interruptible, so a
// query arriving mid-analysis waits for it — the page's debounce keeps
// that collision rare, and completion still answers from the previous
// snapshot the moment the pass finishes.

import { loadAnalyzer } from "../lib/analyzer.mjs";

let analyzer = null;

self.onmessage = async (e) => {
  const { id, op, args } = e.data;
  try {
    if (op === "init") {
      const bytes = await fetch(args.wasmUrl).then((r) => {
        if (!r.ok) throw new Error(`wasm fetch: ${r.status}`);
        return r.arrayBuffer();
      });
      analyzer = await loadAnalyzer(bytes, {
        onStderr: (s) => console.warn("[wasm]", s),
      });
      self.postMessage({ id, result: true });
      return;
    }
    if (!analyzer) throw new Error("worker not initialized");
    let result;
    if (op === "analyze") {
      const t0 = performance.now();
      result = analyzer.analyze(args.src);
      result.elapsed_ms = Math.round(performance.now() - t0);
    } else if (op === "complete") {
      result = analyzer.complete(args.path, args.text, args.line, args.character);
    } else if (op === "typeAt") {
      result = analyzer.typeAt(args.path, args.line, args.character);
    } else if (op === "related") {
      result = analyzer.relatedFiles(args.path);
    } else if (op === "traceroute") {
      result = analyzer.traceroute(args.query);
    } else if (op === "traceTargets") {
      result = analyzer.traceTargets();
    } else {
      throw new Error(`unknown op: ${op}`);
    }
    self.postMessage({ id, result });
  } catch (err) {
    self.postMessage({ id, error: String(err?.message || err) });
  }
};
