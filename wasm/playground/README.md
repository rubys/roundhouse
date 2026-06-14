# Playground (rung A, Phase 1)

The multi-target playground from `docs/browser-demo-plan.md` Phase 1: an
editable Monaco source tree → in-browser `transpile` (wasm) → emitted output,
with a target dropdown. No WebContainer, no npm, no bundler.

Builds directly on the Phase 0 spike (`../browser-spike/`): it reuses that
dir's `transpile.mjs` driver and its already-generated `roundhouse_wasm.wasm`
+ `fixture.json`. What's net-new over the spike is the **editable** loop —
source tree, editor, debounced edit → re-transpile.

## Files

| File | Role | Tracked |
|---|---|---|
| `index.html` | three-pane layout (sources / editor / output) | yes |
| `editor.js` | editor abstraction: Monaco via CDN, `<textarea>` fallback | yes |
| `playground.js` | app: tree, debounced transpile loop, output, test hooks | yes |
| `verify-playground.mjs` | Playwright: drive edit→transpile→render in chromium | yes |
| `playground.png` | screenshot from the verifier | no (gitignored) |

## Run

Serve the **`wasm/` directory** (one level up — so `../browser-spike/`
imports + fetches resolve), then open `/playground/`:

```sh
# from wasm/ (after the spike's artifacts exist — see ../browser-spike/README.md):
#   ../browser-spike/roundhouse_wasm.wasm and ../browser-spike/fixture.json
python3 -m http.server 8099        # run from wasm/
open http://localhost:8099/playground/

# automated smoke check (chromium via tests/browser_smoke/node_modules):
node playground/verify-playground.mjs
```

## What works

- Source tree seeded from real-blog (`.rb` / `.erb`), click to edit.
- Edit → 250 ms debounce → `transpile(target, srcMap)` → output tree + code.
- Target dropdown over the six wasm-wired backends
  (`typescript|go|rust|python|elixir|crystal`); switching re-transpiles live.
- Monaco loads from a CDN; offline / headless / strict-CSP falls back to a
  `<textarea>` (the loop is identical through both — see `editor.js`).

## Known gaps / follow-ons

- **Not yet packaged as a self-contained deploy dir.** It currently imports +
  fetches from `../browser-spike/`, so it must be served from `wasm/` root.
  Promoting the shared driver + artifacts into a deploy bundle (for
  `rubys.github.io/roundhouse/playground/`) is follow-on packaging.
- **ruby / spinel / kotlin / swift** are not in the target list — they aren't
  wired into `wasm/src/lib.rs`'s `match` yet (a one-line addition each).
- **Output is read-only `<pre>`** (no syntax highlight); Monaco-for-output is
  optional polish.
- Phase 2 (all-targets-at-once columns) and Phase 3 (diagnostics/inferred-type
  Monaco markers) build on this; Phase 3 needs the wasm contract extended to
  return diagnostics (see the plan).
