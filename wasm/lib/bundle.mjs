// In-browser TS→JS bundler for /studio/ (rung D, Phase 4). Turns the
// wasm-emitted worker-profile TypeScript — an in-memory { path: content } map —
// into browser-loadable ESM bundles using esbuild-wasm with a virtual-FS +
// CDN-external plugin. No npm, no container, no service worker.
//
// Why bundle (not bare type-strip + importmap): the app is a ~50-file, ~140-edge
// relative module graph across 3 entry points (main thread + SharedWorker + DB
// worker). Type-strip alone leaves the relative graph unresolved, and workers
// can't use importmaps — so npm deps must be baked in as full URLs. esbuild
// resolves the relative graph in-memory and leaves the two npm deps
// (@hotwired/turbo, @sqlite.org/sqlite-wasm) as full CDN URLs, which resolve in
// any context (main or worker). See docs/browser-demo-plan.md Phase 4.
//
// esbuild itself is loaded from a CDN (like Monaco in editor.js), so nothing is
// vendored; bundling runs entirely client-side via esbuild's wasm.

import { TEST_VIRTUALS, TEST_OVERRIDES, testEntries } from "./test-runtime.mjs";

const ESBUILD_VERSION = "0.28.1";
const ESBUILD_BASE = `https://cdn.jsdelivr.net/npm/esbuild-wasm@${ESBUILD_VERSION}`;

// npm runtime deps in the worker-profile app → ESM CDN URLs. These survive the
// bundle as `external` full URLs (worker-safe, no importmap needed).
export const DEFAULT_CDN = {
  "@hotwired/turbo": "https://esm.sh/@hotwired/turbo@8",
  "@sqlite.org/sqlite-wasm": "https://esm.sh/@sqlite.org/sqlite-wasm@3.47.0-build1",
};

// The worker-profile entry points (relative to the emitted project root).
export const ENTRY_POINTS = ["main.ts", "worker.ts", "src/db_worker.ts"];

let esbuildPromise = null;
function loadEsbuild() {
  if (esbuildPromise) return esbuildPromise;
  esbuildPromise = (async () => {
    const esbuild = await import(`${ESBUILD_BASE}/esm/browser.min.js`);
    await esbuild.initialize({ wasmURL: `${ESBUILD_BASE}/esbuild.wasm` });
    return esbuild;
  })();
  return esbuildPromise;
}

// Normalize a slash path, collapsing "." and ".." segments.
function norm(p) {
  const out = [];
  for (const seg of p.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") out.pop();
    else out.push(seg);
  }
  return out.join("/");
}

// Resolve a relative/absolute import (which uses `.js` specifiers) to a key in
// the in-memory srcMap (where files are `.ts`).
function resolveVfs(srcMap, importer, spec) {
  const p = spec.startsWith("/")
    ? spec.slice(1)
    : norm((importer ? importer.replace(/[^/]*$/, "") : "") + spec);
  const cands = [];
  if (p.endsWith(".js")) cands.push(p.slice(0, -3) + ".ts");
  cands.push(p, p + ".ts", p + "/index.ts");
  for (const c of cands) if (srcMap[c] != null) return c;
  return null;
}

function loaderFor(p) {
  if (p.endsWith(".json")) return "json";
  return "ts";
}

// virtuals: { <bare-specifier>: <source> } resolved in-bundle instead of as a
// CDN external. The test bundle uses it for the `node:test` / `node:assert`
// browser shims (see test-runtime.mjs); the app bundle passes none.
function makeVfsPlugin(srcMap, cdn, virtuals = {}) {
  return {
    name: "studio-vfs",
    setup(build) {
      // CSS is not bundled through JS: studio links the prebuilt tailwind.css
      // separately, so `import "./styles.css"` becomes an empty no-op module
      // (also avoids descending into the Tailwind `@import` directive).
      build.onResolve({ filter: /\.css$/ }, (args) => ({ path: args.path, namespace: "css-stub" }));
      build.onLoad({ filter: /.*/, namespace: "css-stub" }, () => ({ contents: "", loader: "js" }));

      build.onResolve({ filter: /.*/ }, (args) => {
        if (args.kind === "entry-point") {
          return srcMap[args.path] != null
            ? { path: args.path, namespace: "vfs" }
            : { errors: [{ text: `entry not found: ${args.path}` }] };
        }
        // Bare specifier → virtual shim, else CDN external (full URL in output).
        if (!args.path.startsWith(".") && !args.path.startsWith("/")) {
          if (virtuals[args.path] != null) return { path: args.path, namespace: "virtual" };
          if (cdn[args.path]) return { path: cdn[args.path], external: true };
          return { errors: [{ text: `unmapped npm import: ${args.path} (from ${args.importer})` }] };
        }
        const r = resolveVfs(srcMap, args.importer, args.path);
        return r
          ? { path: r, namespace: "vfs" }
          : { errors: [{ text: `cannot resolve ${args.path} from ${args.importer}` }] };
      });

      build.onLoad({ filter: /.*/, namespace: "virtual" }, (args) => ({
        contents: virtuals[args.path],
        loader: "ts",
      }));

      build.onLoad({ filter: /.*/, namespace: "vfs" }, (args) => ({
        contents: srcMap[args.path],
        loader: loaderFor(args.path),
      }));
    },
  };
}

// Load (once) and return a bundler. bundle(srcMap, entryPoints?) returns
// { ms, errors, warnings, outputs: { <entryBasename>: {path, text, bytes} } }.
export async function loadBundler(opts = {}) {
  const esbuild = await loadEsbuild();
  const cdn = opts.cdn || DEFAULT_CDN;
  return {
    // base: the app's deploy path (becomes import.meta.env.BASE_URL — the worker
    // runtime strips it from routes). Defaults to "/".
    async bundle(srcMap, entryPoints = ENTRY_POINTS, { base = "/" } = {}) {
      const t0 = performance.now();
      let result;
      try {
        result = await esbuild.build({
          entryPoints,
          bundle: true,
          format: "esm",
          target: "es2022",
          outdir: "out",
          write: false,
          metafile: true,
          // The emitted client reads import.meta.env.BASE_URL (vite injects it
          // in the normal build). Nothing else in the runtime reads
          // import.meta.env, so defining the whole object is safe.
          define: { "import.meta.env": JSON.stringify({ BASE_URL: base }) },
          plugins: [makeVfsPlugin(srcMap, cdn)],
          logLevel: "silent",
        });
      } catch (e) {
        // esbuild throws on build failure; surface its formatted errors.
        return { ms: performance.now() - t0, errors: e.errors || [{ text: String(e) }], warnings: [], outputs: {} };
      }
      const outputs = {};
      for (const f of result.outputFiles) {
        const name = f.path.replace(/^.*\//, ""); // basename (main.js, worker.js, db_worker.js)
        outputs[name] = { path: f.path, text: f.text, bytes: f.contents.length };
      }
      return { ms: performance.now() - t0, errors: result.errors, warnings: result.warnings, outputs };
    },

    // Bundle the emitted worker-profile test suite into ONE standalone ESM
    // bundle PER SPEC FILE (rung D.2, Phase 7). `emitted` is the same
    // { path: content } map `bundle()` takes; this overrides src/db.ts +
    // src/juntos.ts with the in-memory test runtime, adds the node:test /
    // node:assert shims, and synthesizes one entry per spec. Each output runs
    // in its own Worker (fresh in-memory DB) — see studio.js `runTests`, which
    // gives the per-file isolation `node --test` uses in CI.
    // Returns { ms, errors, warnings, outputs: [{ spec, text, bytes }] }.
    async bundleTests(emitted, { base = "/" } = {}) {
      const t0 = performance.now();
      const specs = Object.keys(emitted)
        .filter((p) => /^test\/.*\.test\.ts$/.test(p))
        .sort();
      if (specs.length === 0) {
        return { ms: 0, errors: [{ text: "no emitted test specs found" }], warnings: [], outputs: [] };
      }
      const entries = testEntries(specs);
      const src = { ...emitted, ...TEST_OVERRIDES };
      for (const e of entries) src[e.path] = e.source;
      let result;
      try {
        result = await esbuild.build({
          entryPoints: entries.map((e) => e.path),
          bundle: true,
          format: "esm",
          target: "es2022",
          outdir: "out",
          write: false,
          define: { "import.meta.env": JSON.stringify({ BASE_URL: base }) },
          plugins: [makeVfsPlugin(src, cdn, TEST_VIRTUALS)],
          logLevel: "silent",
        });
      } catch (e) {
        return { ms: performance.now() - t0, errors: e.errors || [{ text: String(e) }], warnings: [], outputs: [] };
      }
      const outputs = entries.map((e) => {
        const baseName = e.path.replace(/^.*\//, "").replace(/\.ts$/, ".js");
        const f = result.outputFiles.find((o) => o.path.replace(/^.*\//, "") === baseName);
        return { spec: e.spec, text: f ? f.text : null, bytes: f ? f.contents.length : 0 };
      });
      return { ms: performance.now() - t0, errors: result.errors, warnings: result.warnings, outputs };
    },
  };
}
