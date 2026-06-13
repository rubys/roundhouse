# Browser smoke tests — SharedWorker target

Playwright spec that exercises roundhouse's TypeScript+SharedWorker
deployment target end-to-end against the `real-blog` fixture:

1. **`scripts/prebuild.mjs`** (chained from `npm test`) runs
   `cargo run --bin emit_preview --target typescript --profile worker
   --out .emitted/ fixtures/real-blog`, then `npm install` and
   `npm run build` inside `.emitted/`. Must complete before
   Playwright loads the config — Playwright validates `webServer.cwd`
   at load time.
2. **`playwright.config.ts`** runs `npm run preview` against the
   built `dist/` and points Chromium at `http://localhost:5173`.
3. **`tests/worker_target.spec.ts`** loads the page, waits for the
   SharedWorker bridge to reach ready, then opens its own
   SharedWorker port and drives the CRUD surface — GET (index),
   POST (create → insert → redirect), POST with invalid params
   (422 re-render), DELETE via `_method` override (destroy) — plus
   a multi-tab BroadcastChannel broadcast. Each probe asserts no 5xx
   and the expected status/redirect.

The synthetic-fetch path matters: when the SharedWorker errors with
5xx, Turbo Drive falls back to a full-page navigation against vite
preview's SPA fallback (which serves the same `index.html`),
masking the underlying error. By probing the SharedWorker directly
we surface the real controller error string in the failure message.

## Running

First-time setup (downloads ~120 MB Chromium):

```bash
cd tests/browser_smoke
npm install
npm run install-browsers
```

Run the suite:

```bash
npm test                  # full pipeline: emit + build + serve + drive
SKIP_EMIT=1 npm test      # reuse existing .emitted/ if present
```

Iterate on a single spec while keeping the preview server up:

```bash
# Terminal 1 (once):
npm test                  # leaves preview server running on :5173

# Terminal 2 (repeat as needed):
SKIP_EMIT=1 npm run test-only       # skip prebuild + reuse the existing server
```

`reuseExistingServer` is true outside CI; the second invocation
just re-runs the spec against the already-running preview server.

## What this catches

Framework-runtime portability gaps in the transpiled output: Node
globals (`Buffer`, `process`, `require`) referenced from emitted
TypeScript that runs inside a SharedWorker; missing browser
polyfills; broken MessagePort / BroadcastChannel wiring; `installDb`
ordering bugs; and a Node-only `Db` variant leaking into the browser
bundle (the worker `src/db.ts` must proxy to the dedicated DB Worker,
not import `better-sqlite3` / `@libsql/client` — a regression this
suite caught after it shipped silently for a month).

Driving every CRUD verb through the SharedWorker also covers the
request surfaces a portability gap tends to hide in: form-encoded
body parsing (POST/DELETE), the adapter's MessagePort round-trip on
write (insert / update / delete, not just read), validation failure
(422), the `_method` override branch, and a Turbo Stream broadcast
prepending into a second tab's DOM end-to-end (the rendered
`turbo_stream_from` subscription, not just the raw channel).

## What this doesn't catch (yet)

- Visual regressions (would need Playwright screenshot comparison
  or Vitest browser mode with `expect(page).toHaveScreenshot()`).
- Importmap asset wiring — the specs deliberately ignore `/assets/*.js`
  404s (Stimulus/Turbo load via the Vite bundle, not the importmap),
  so a broken importmap pin wouldn't turn the suite red.
- OPFS persistence across reloads — the specs never reload the page,
  so "data written through the DB Worker survives a reload" is
  unverified.
- Active Storage / file uploads — deferred in `db_worker.ts` (the
  `file:*` message ops are intentionally unimplemented).

These are natural extensions; the harness is shaped to grow into
them without restructuring.
