# Browser spike (rung A, Phase 0)

De-risk spike for `docs/browser-demo-plan.md`: proves the roundhouse compiler
runs **in a browser tab** — no WebContainer, no npm, no bundler. Three static
files (`.mjs` + `.wasm` + `.json`) served over plain HTTP.

Key finding: the `wasi_snapshot_preview1` shim from `../test-node.mjs` uses
only web-platform APIs, so the *same* `wasi-shim.mjs` + `transpile.mjs` drive
both the Node validator and the browser page. No `@bjorn3/browser_wasi_shim`
needed.

## Files

| File | Role | Tracked |
|---|---|---|
| `wasi-shim.mjs` | minimal WASI shim (Node + browser) | yes |
| `transpile.mjs` | C-ABI driver → `transpile(lang, srcMap)` | yes |
| `gen-fixture.mjs` | build `fixture.json` from real-blog (text files only) | yes |
| `validate-fixture.mjs` | Node: assert fixture → 15 TS files | yes |
| `verify-browser.mjs` | Playwright: drive the page in chromium | yes |
| `spike.js` / `index.html` | the browser page | yes |
| `roundhouse_wasm.wasm` | copied compiler artifact | no (gitignored) |
| `fixture.json` | generated fixture | no (gitignored) |

## Reproduce

```sh
# 1. Build the compiler wasm (needs rustup + WASI SDK; see plan task #5).
#    Until then, copy the committed artifact:
cp ../target/wasm32-wasip1/release/roundhouse_wasm.wasm .

# 2. Generate the fixture and validate it under Node.
node gen-fixture.mjs       # → fixture.json (125 files, ~152 KB)
node validate-fixture.mjs  # → full-stack TS emit (~79 files), exits 0

# 3. Serve + open in a browser.
python3 -m http.server 8099   # then visit http://localhost:8099/

# 4. (optional) Automated browser check.
python3 -m http.server 8099 &
node verify-browser.mjs    # chromium via tests/browser_smoke/node_modules
```

## Measured (localhost, chromium headless)

real-blog → emitted files, live target switching, no console errors. File
counts track the emit pipeline (full-stack: app + runtime + tests +
per-file sourcemaps + config), so they grow as emit gains features — the
tests assert a floor + key-file presence, not an exact count:

| target | files | transpile |
|---|---|---|
| typescript | 79 | ~22 ms (cold) |
| crystal | 54 | ~5 ms |
| rust | 49 | ~7 ms |
| go | 48 | ~5 ms |
| elixir | 38 | ~5 ms |
| python | 34 | ~5 ms |

Cold transpile pays a one-time wasm warmup; subsequent calls are ~5 ms.
(Counts as of 2026-06-14, current-main emit; native `emit_preview` and the
wasm path produce the identical 79-file TS set.)
