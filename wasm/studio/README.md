# Studio (rung D)

"The blog, editable" — the rung D surface from `docs/browser-demo-plan.md`: edit
Ruby → recompile in-browser → run the emitted **TypeScript** blog live against a
sqlite-wasm database, no server and no container. TypeScript-only, because it is
the one target with a browser runtime (decisions #5/#6); there is no target
dropdown.

Studio shares the editor, source tree, compiler driver, and seed app with
`/playground/` via `../lib/` (see `../lib/README.md`); the difference is the
right-hand pane — playground shows emitted **code**, studio shows the **running
app**.

## Status — Phase 4 done

Working today (the whole client-side **edit → compile → bundle** chain):

- Shared `../lib/` editor + source tree + the debounced edit loop, proven on a
  second surface.
- Every edit transpiles Ruby → the **worker-profile** TypeScript in the browser
  (wasm `profile: "worker"` — the runnable SharedWorker app: `main.ts`,
  `worker.ts`, `src/db_worker.ts`, …), then bundles it to 3 browser-loadable
  ESM files via **esbuild-wasm** (`../lib/bundle.mjs`). The app pane shows live
  transpile + bundle readouts (file counts, sizes, ms).
- Diagnostics squiggles on the open file (same inference overlay as playground).

esbuild loads from a CDN (like Monaco), so nothing is vendored; if that load
fails (offline / strict CSP) studio degrades to transpile-only.

Not yet (Phase 5, the next step):

- **Run** the app: load those 3 bundles as the live app (main thread +
  SharedWorker + DB worker) over sqlite-wasm — reusing the blog's runtime
  (`runtime/typescript/sqlite_wasm_engine.ts`, `db_worker.ts`, `juntos*.ts`) —
  and hot-swap on edit. The app pane shows that roadmap + the live bundle status
  instead of the app.

## Files

| File | Role | Tracked |
|---|---|---|
| `index.html` | three-pane layout (sources / editor / running app) | yes |
| `studio.js` | app: source tree, debounced transpile+bundle loop, app-pane readouts, test hooks | yes |
| `verify-studio.mjs` | Playwright: drive the chain in chromium (needs network for the CDNs) | yes |
| `studio.png` | screenshot from the verifier | no (gitignored) |

Shared assets (compiler driver, editor, tree, **bundler** `bundle.mjs`, binary,
fixture) live in `../lib/`.

## Run

Serve the **parent** (`wasm/`) as the web root (the page imports `../lib/`):

```sh
python3 -m http.server 8099        # run from wasm/
open http://localhost:8099/studio/

# automated smoke check (chromium via tests/browser_smoke/node_modules):
cd studio && node verify-studio.mjs
```

## Publishing

The CI `build-site` job copies this dir into `_site/studio/` alongside
`_site/lib/` and `_site/playground/`, so the surface lands at
`rubys.github.io/roundhouse/studio/`. See the "Publishing to Pages" section of
`docs/browser-demo-plan.md`.
