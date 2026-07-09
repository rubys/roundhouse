// Main-thread client for the shared compiler worker (worker.mjs). Spawns the
// worker, does the {id, op, args} RPC, and — because a wasm call can HANG (an
// emitter looping on a survey-degraded app) or TRAP — guards every call with a
// watchdog. On timeout or a worker error it terminates the worker, respawns a
// fresh one (re-loading the wasm), and rejects the in-flight call so the
// surface renders a diagnostic instead of freezing the tab.
//
// The worker holds no durable state: the app source lives on the main thread
// and is re-sent on the next call, so a restart is transparent apart from the
// one call that provoked it. This is the shared substrate both /playground/
// (transpile) and /ide/ (analyze) run on.
export function createClient({
  workerUrl,
  wasmUrl,
  timeoutMs = 30000,
  initTimeoutMs = 60000,
  onRestart,
} = {}) {
  let worker;
  let pending;
  let nextId;
  let ready; // init promise for the CURRENT worker; reassigned on every spawn

  function spawn() {
    worker = new Worker(workerUrl, { type: "module" });
    pending = new Map();
    nextId = 1;
    worker.onmessage = (e) => {
      const { id, result, error } = e.data;
      const p = pending.get(id);
      if (!p) return;
      pending.delete(id);
      clearTimeout(p.timer);
      error ? p.reject(new Error(error)) : p.resolve(result);
    };
    worker.onerror = (ev) => restart(`worker crashed: ${ev?.message || "trap"}`);
    ready = post("init", { wasmUrl }, initTimeoutMs);
    // A failed init shouldn't throw an unhandled rejection before a caller
    // awaits it; callers observe the failure through `call`.
    ready.catch(() => {});
  }

  function restart(reason) {
    const dead = worker;
    for (const [, p] of pending) {
      clearTimeout(p.timer);
      p.reject(new Error(reason));
    }
    try { dead.terminate(); } catch { /* already gone */ }
    if (onRestart) { try { onRestart(reason); } catch { /* ignore */ } }
    spawn();
  }

  // Post one message to the CURRENT worker with a watchdog. A fired watchdog
  // rejects the call and triggers a restart (the wasm can't be interrupted, so
  // tearing the worker down is the only way to reclaim the thread).
  function post(op, args, timeout) {
    return new Promise((resolve, reject) => {
      const id = nextId++;
      const timer = setTimeout(() => {
        if (!pending.has(id)) return;
        pending.delete(id);
        reject(new Error(`${op} exceeded ${timeout}ms`));
        restart(`${op} timed out after ${timeout}ms`);
      }, timeout);
      pending.set(id, { resolve, reject, timer });
      worker.postMessage({ id, op, args });
    });
  }

  // Ensure the current worker is initialized, then post. If a restart swaps
  // `ready` out from under us mid-await (a concurrent call timed out), retry
  // against the fresh worker rather than posting into a half-dead one.
  async function call(op, args) {
    for (let attempt = 0; attempt < 3; attempt++) {
      const r = ready;
      try {
        await r;
      } catch (e) {
        if (r === ready) throw e; // this worker's init genuinely failed
        continue; // restarted during await → retry with the new worker
      }
      if (r === ready) return post(op, args, timeoutMs);
    }
    throw new Error("compiler worker unavailable");
  }

  spawn();

  return {
    ready: () => ready,
    call,
    transpile: (language, src, opts = {}) =>
      call("transpile", opts.profile ? { language, src, profile: opts.profile } : { language, src }),
    analyze: (src) => call("analyze", { src }),
    complete: (path, text, line, character) => call("complete", { path, text, line, character }),
    typeAt: (path, line, character) => call("typeAt", { path, line, character }),
    related: (path) => call("related", { path }),
    traceroute: (query) => call("traceroute", { query }),
    traceTargets: () => call("traceTargets", {}),
    dispose: () => { try { worker.terminate(); } catch { /* ignore */ } },
  };
}
