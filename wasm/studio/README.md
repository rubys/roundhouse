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

## Status — Phase 4 scaffold

This is the Phase 4 scaffold. Working today:

- Shared `../lib/` editor + source tree + the debounced **edit → transpile**
  loop, proven on a second surface. Every edit recompiles to TypeScript in the
  browser; the app pane shows a live build readout (file count + ms + errors).
- Diagnostics squiggles on the open file (same inference overlay as playground).

Not yet (Phase 5, the next step):

- **Run** the app. That needs `esbuild-wasm` to bundle the emitted TS, then a
  hot-swap into the blog's SharedWorker + sqlite-wasm runtime
  (`runtime/typescript/sqlite_wasm_engine.ts`, `db_worker.ts`, `juntos*.ts`).
  The app pane currently shows that roadmap + the live build status instead of
  the app.

## Files

| File | Role | Tracked |
|---|---|---|
| `index.html` | three-pane layout (sources / editor / running app) | yes |
| `studio.js` | app: source tree, debounced transpile loop, app-pane build readout, test hooks | yes |
| `verify-studio.mjs` | Playwright: drive the scaffold in chromium | yes |
| `studio.png` | screenshot from the verifier | no (gitignored) |

Shared assets (driver, editor, tree, binary, fixture) live in `../lib/`.

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
