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

function makeVfsPlugin(srcMap, cdn) {
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
        // Bare specifier → CDN external (kept as a full URL in the output).
        if (!args.path.startsWith(".") && !args.path.startsWith("/")) {
          if (cdn[args.path]) return { path: cdn[args.path], external: true };
          return { errors: [{ text: `unmapped npm import: ${args.path} (from ${args.importer})` }] };
        }
        const r = resolveVfs(srcMap, args.importer, args.path);
        return r
          ? { path: r, namespace: "vfs" }
          : { errors: [{ text: `cannot resolve ${args.path} from ${args.importer}` }] };
      });

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
    async bundle(srcMap, entryPoints = ENTRY_POINTS) {
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
  };
}
