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
   SharedWorker port and sends a synthetic `fetch` message to
   `/articles`. Asserts the response status is < 500.

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
ordering bugs.

## What this doesn't catch (yet)

- Visual regressions (would need Playwright screenshot comparison
  or Vitest browser mode with `expect(page).toHaveScreenshot()`).
- Multi-tab BroadcastChannel coordination (would need a multi-page
  Playwright spec).
- Form submission edge cases (need a POST probe).

These are natural extensions; the harness is shaped to grow into
them without restructuring.
