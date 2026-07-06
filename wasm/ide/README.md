# /ide/ — the analyzer as a browser IDE

Monaco over the roundhouse analyzer running in a Web Worker, preloaded
with a real Rails app. Everything on screen is whole-program inference —
no server, no app boot, no annotations:

- **hover** — inferred type at the cursor (works inside ERB/HAML
  templates too; the analyzer's view spans point into the template files)
- **completion** — typed members/scopes/column-kwargs/ivars from the
  last-good snapshot (`ide::complete_at`, the same core the LSP uses)
- **markers** — diagnostics with the coverage ledger: `info`-severity
  notes mean "roundhouse can't see this yet", not "your code is wrong"
- **⌘P / Ctrl+P** — fuzzy file + class picker
- **⌘⇧R** — related files, from the *inferred* render graph
  (`view_feeders`/`render_edges`) and include edges — not filename
  conventions
- **coverage** button — the ingest-gap punch list

Edits re-analyze in the worker (~2.5s for Mastodon, debounced); queries
answer from the previous snapshot meanwhile.

## Running locally

```sh
WASI_SDK_PATH=/opt/wasi-sdk ./build.sh          # from wasm/ — builds lib/roundhouse_wasm.wasm
node bundle-src.mjs ~/git/mastodon app-src.json \
  --name mastodon --open app/controllers/statuses_controller.rb   # from wasm/ide/
python3 -m http.server 8099                      # from wasm/ (the page imports ../lib/)
open http://localhost:8099/ide/
```

Any Rails app works as the bundle; the published site ships Mastodon at
the SHA pinned in `.github/workflows/ci.yml` (`MASTODON_SHA`), with the
app's LICENSE and commit embedded in `app-src.json` (AGPL source
redistribution, the compliant kind).

## Verifying

`verify-ide.mjs` drives the page in headless chromium (Playwright, the
`tests/browser_smoke` install) and asserts the demo beats: typed hover
(incl. in HAML), typed completion, related files, coverage ledger. CI
runs it as `browser-smoke-ide` against the same pinned bundle it
publishes, so the demo can't silently regress.

```sh
node verify-ide.mjs        # from wasm/ide/, with the server above running
```
