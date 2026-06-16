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

## Status — Phase 5 done (full-reload loop)

Studio runs the emitted blog **live**, entirely client-side:

> edit Ruby → wasm transpile (worker profile) → esbuild bundle → host in a
> service worker → run the app in an iframe over sqlite-wasm → edit again → the
> running app reflects it.

- The right pane is the **running app**, not code: a service worker (`sw.js`)
  serves the esbuild bundles + an HTML shell at a same-origin scope
  (`<studio>/app/`), and an iframe mounts it there so `new SharedWorker`/
  `new Worker` + module loads + routes resolve from real URLs
  (`../lib/app-host.mjs` registers the SW + drives the iframe).
- Every edit re-bundles and reloads the iframe. Bundle URLs carry a per-build
  `?v=` so a fresh SharedWorker mints each build (they're URL-keyed; the worker
  is where rendering happens). The app's OPFS DB persists across reloads.
- **OPFS is namespaced per deploy path** (`import.meta.env.BASE_URL`), so the
  studio app instance and the standalone `/blog/` never share a pool.
- esbuild + Monaco + sqlite-wasm/turbo + Tailwind load from CDNs; each piece
  degrades independently (no esbuild → transpile-only; no SW → no run).

**Phases 6-7 (rung D.2) done — the Minitest suite ships AND runs in-browser.**
Every build's worker-profile transpile emits the suite (`test/<x>.test.ts`, the
`test/_runtime/` harness, `test/fixtures/*.ts`); `testSuiteFrom()` retains it
(status line shows a `· N test suites` count, exposed via
`window.__studio.testSuite()`). `window.__studio.runTests()` then **runs** it:

> bundle the suite (one standalone ESM per spec file, via `bundleTests`) →
> run each file in its own Worker over a FRESH in-memory sqlite-wasm DB →
> aggregate `{total,passed,failed,skipped,results}`.

- The browser has no `node:test`/`node:assert`; `../lib/test-runtime.mjs`
  supplies shims + an in-memory `Db` + `setupTestDb`, injected at **bundle
  time** (esbuild overrides `src/db.ts`/`src/juntos.ts`, virtual `node:*`), so
  the emitted suite stays **byte-identical to CI** — the in-browser run and the
  CI run are provably the same suite. No emitter/compiler change.
- **DB isolation (risk #4):** the engine's `opfs:false` path opens a throwaway
  in-memory DB — never the live app's opfs pool. One worker per spec file gives
  the per-file isolation `node --test` uses in CI (a spec that mutates fixtures,
  e.g. `ArticleTest#test_destroys_comments`, can't leak into the next file).
- A run fires once in the background after boot (console-logged); the test
  *sources* are editable in the tree, so an edit re-runs green/red.

Deferred: true module hot-swap (no reload); the **results UI panel** + per-edit
re-run + cross-target CI badge strip (Phase 8); runtime sourcemaps so a failing
test clicks back to the Ruby line (Phase 9).

## Files

| File | Role | Tracked |
|---|---|---|
| `index.html` | three-pane layout (sources / editor / running app iframe) | yes |
| `studio.js` | the loop: source tree, transpile→bundle→host, app shell, emitted-test-suite retention (Phase 6) + in-browser run (Phase 7, `runTests`), test hooks | yes |
| `sw.js` | app-host service worker: serves the in-memory app at its scope | yes |
| `verify-studio.mjs` | Playwright: boot → run → edit-reflects, in chromium (needs network) | yes |
| `studio.png` | screenshot from the verifier | no (gitignored) |

Shared infra (compiler driver, editor, tree, bundler `bundle.mjs`, app-host
`app-host.mjs`, the compiler wasm, fixture) lives in `../lib/`.

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
