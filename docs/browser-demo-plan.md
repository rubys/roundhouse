# Browser demo plan

Build a family of in-browser demos that run the roundhouse compiler — and,
for the TypeScript target, the *emitted application and its test suite* —
entirely client-side, with **no WebContainer and no server**. The compiler
is already a self-contained wasm module; the TS runtime already runs a real
full-stack app in the browser on sqlite-wasm. This plan stitches those two
existing assets into an interactive editor (rungs A–C), then into a live
edit→compile→render loop (rung D), then closes the development cycle with an
in-browser test runner (rung D.2).

## The no-container thesis (why this is structurally cheaper than ruby2js)

ruby2js's editor (`ruby2js.github.io/ruby2js/editor`) boots a StackBlitz
**WebContainer** — Node + npm + a virtual FS emulated in wasm — because the
ruby2js compiler is written in Ruby and needs a runtime to execute, and the
editor also runs vite inside the container to build/run the output.
WebContainer requires `SharedArrayBuffer`, which means COOP/COEP
cross-origin-isolation headers, and it is proprietary.

roundhouse needs none of that:

- **The compiler is Rust → wasm, and already builds.** `wasm/Cargo.toml`
  produces a `cdylib`; the artifact builds to
  `wasm/target/wasm32-wasip1/release/roundhouse_wasm.wasm` (~3.2 MB, carrying
  the real rbs FFI parser — see the blocker note below).
  `wasm/src/lib.rs` exposes exactly the playground API: a C-ABI
  `transpile(ptr,len) -> u64` (packed out-ptr/out-len) over a
  `{language, src:{path→content}} → {language, files:[{path,content}]}`
  JSON contract, routing to `typescript|rust|crystal|python|elixir|go`.
- **The old rbs/libclang wasm blocker is resolved — cleanly, via upstream.**
  roundhouse now depends on the real published `ruby-rbs = "0.3"` on **all**
  targets (no more per-arch split, no `rubys/rbs-rust` fork). The wasm build
  only needs `ruby-rbs-sys` built for `wasm32`: its vendored RBS C parser
  compiled by the WASI-SDK clang, and its bindgen run against the **host**
  target (`#[repr(C)]` layouts are portable — sidestepping the
  libclang-version-fragile bindgen-against-wasm failures). That build.rs
  support is upstreamed as **ruby/rbs#2992** (`rust-wasm-bindings`), mirroring
  `ruby-prism-sys`. Until it merges and publishes, `wasm/Cargo.toml` carries a
  temporary `[patch.crates-io] ruby-rbs-sys = { path = ... }` pointing at a
  local copy of that build; repin to the published crate once #2992 lands.
  Build requirement: `WASI_SDK_PATH` (the WASI SDK lives at `/opt/wasi-sdk`).
- **The emitted app already runs in the browser without cross-origin
  isolation.** The live blog (`rubys.github.io/roundhouse/blog/`) runs
  Ruby→TS (`worker` profile) on `@sqlite.org/sqlite-wasm` via the
  opfs-sahpool VFS, which deliberately avoids `SharedArrayBuffer` — so it
  ships on **plain GitHub Pages, no COOP/COEP**. WebContainer cannot drop
  those headers.

What a WebContainer would still buy that bare wasm does not: running real
Node tooling live on the output — `npm install`, the actual `tsc`/`vitest`/
`vite` dev server. **None of the demos below need that** (the blog demo
pre-bundles, and the test runner uses an in-browser harness — see D.2), so a
container never earns its weight here.

## Demo arc (recommended order)

| Rung | Demo | What it shows | New work | Container? |
|---|---|---|---|---|
| **A** | Multi-target playground (`/playground/`) | Edit Ruby → pick TS/Go/Rust/Py/Elixir/Crystal → emitted code | Monaco + load wasm + WASI-in-browser shim | No |
| **C** | Inference / diagnostics overlay (in `/playground/`) | Monaco markers for inferred types + unsupported-feature diagnostics | expose analyzer/diagnostics through wasm | No |
| **D** | Live app loop — **separate surface `/studio/`** | Edit Ruby → recompile in-browser → hot-swap the running TS blog | shared `lib/` factor-out + esbuild-wasm TS→JS + hot-reload wiring | No |
| **D.2** | In-browser test runner (in `/studio/`) | …→ run the emitted Minitest suite → green/red → click failure back to Ruby | node:test→browser harness + results panel + runtime sourcemaps | No |

> **Dropped: rung B ("all-targets-at-once").** Each target is a whole idiomatic
> project (34–79 files) that *restructures* — different dir layouts, views as
> functions vs files, bundled models, extra test files — so there's no reliable
> cross-target file correspondence to lay side by side, and a six-project grid
> isn't legible. The one durable idea it surfaced (a per-file `source`
> provenance field so a UI could map one source → its output in each target)
> folds into the rung C contract work instead — see Phase 3.

A is the de-risk spike and the clearest "what is this project" demo (the
multi-target angle is the differentiator ruby2js structurally lacks); A+C ship
as `/playground/` (live). C is a UI extension of the same wasm call. **D is the
demo nobody else can build** — it fuses the two assets roundhouse already has
into a live full-stack loop on static hosting — and ships as its own surface
`/studio/` ("the blog, editable"), sharing modules with the playground but not
its UI (decision #6). **D.2 turns "look, it transpiles" into "look, it's a real,
verifiable development cycle."**

## Decisions locked in (provisional — confirm at each rung's Phase 0)

1. **No WebContainer, ever, in this arc.** Every rung loads self-contained
   wasm modules directly. If a future demo genuinely needs live Node tooling,
   that is a separate decision, not a default.
2. **The compiler wasm is the existing `wasm/` crate.** Do not fork a second
   compiler build. The playground's transpile call is the existing
   `transpile(json)→json` contract; extend that contract (e.g. to return
   diagnostics for rung C) rather than adding parallel entry points.
3. **Input is a small project tree, not a single snippet.** roundhouse
   transpiles `app/models/*.rb` + controllers + views together; the wasm
   contract is keyed by path. The editor needs a multi-file tree (or a
   curated default app) — not ruby2js's one-textarea model. Ship a sensible
   default app (the real-blog fixture) and let the file tree be editable.
4. **TS→JS in the browser is `esbuild-wasm`, not a container.** The blog does
   TS→JS at build time via vite; a *live* editor needs it in-browser.
   esbuild ships a wasm build that does transform (type-strip) + bundle as a
   single module — another self-contained wasm alongside the compiler.
   Decision deferred-but-recommended; confirm at D Phase 0 against the
   alternative (emit strippable TS + import maps only).
5. **Live test execution is TS-only; other targets are CI-attested.** Only
   the TS target has a browser runtime. D.2 runs the TS suite live and shows
   the *same* suite's results on Go/Rust/Python/etc. as precomputed CI
   badges. **Do not imply the non-TS runs are live.** This leans into the
   cross-target conformance story (one Minitest suite, every backend) rather
   than hiding the limit.
6. **Rung D is a SEPARATE surface (`/studio/`), not a mode of the playground.**
   Decided 2026-06-14. D shares a lot of code with the playground but tells a
   different story and has a different shape, so it ships as its own published
   surface — "share code, separate surface." Reasons:
   - *Different message.* `/playground/` answers "what does my Ruby compile
     to?" (read output code, compare targets, inspect inferred types).
     `/studio/` answers "watch my Rails app run and change live."
   - *Different output shape.* The playground's right pane is source code;
     studio's is a *running app + a live database*. Not a tab — a different
     surface.
   - *TS-only vs six-target.* The playground is proudly multi-target; D only
     works for TypeScript (the one browser runtime). A "Run" greyed out for 5
     of 6 targets sits wrong inside the playground; standalone, studio is
     unambiguously "the TypeScript app, live."
   - *Weight & risk.* D pulls in esbuild-wasm + the SharedWorker + sqlite-wasm
     + OPFS + hot-swap. Folding that in taxes the lean playground and lets D's
     (riskiest-rung) breakage destabilize an already-shipped surface.

   Studio is conceptually **"the blog, editable"** — it composes the existing
   `/blog/` runtime (`sqlite_wasm_engine.ts`, `db_worker.ts`, `juntos*.ts`) +
   the playground's editor/driver + esbuild glue, so it sits closer to `/blog/`
   than to `/playground/`. The factoring: promote the shared pieces
   (`transpile.mjs` / `wasi-shim.mjs`, `editor.js`, the source-tree + debounced
   edit→transpile loop) into a shared `lib/` both surfaces import — that's
   rung D's Phase 4 first step. `/demo/` is the hub page; it links blog +
   playground + studio so a visitor lands oriented.

## Phase status

| # | Phase | Rung | Days | Status |
|---|---|---|---|---|
| 0 | Audit + WASI-in-browser spike (load `roundhouse_wasm.wasm`, transpile real-blog, render output) | A | ½–1 | **DONE** — see `wasm/browser-spike/` |
| 1 | Monaco editor + multi-file tree + target dropdown → wasm transpile → output pane | A | 1–2 | **DONE & PUBLISHED** — `wasm/playground/`, live at `/playground/` |
| 3 | Extend wasm contract to return diagnostics + inferred types (+ per-file `source` provenance); Monaco markers | C | 1–2 | **DIAGNOSTICS + INFERRED-TYPE HOVERS DONE**; only the `source` provenance field remains |
| 4 | `/studio/` scaffold + shared `lib/` + esbuild-wasm TS→JS bundle step in-browser | D | 1 | **DONE** — shared `lib/`, `/studio/`, wasm `profile` contract, esbuild-wasm bundling all landed |
| 5 | Live loop: edit Ruby → wasm recompile → esbuild bundle → hot-swap running blog | D | 2–3 | unblocked (1,4 done) — next |
| 6 | Emit + ship the Minitest suite into the browser payload | D.2 | ½–1 | blocked on 5 |
| 7 | `node:test` → browser test-runner harness + in-memory sqlite isolation | D.2 | 1–2 | blocked on 6 |
| 8 | Test-results UI panel + cross-target CI badge strip | D.2 | ½–1 | blocked on 7 |
| 9 | Runtime sourcemaps: failing-test stack traces map back to Ruby source | D.2 | 1–2 | blocked on 7 |

Total: rung A alone **~2–3 days**; full arc through D.2 **~9–14 days**.
A is independently shippable and de-risks everything after it.

## Publishing to Pages

The demos publish to `rubys.github.io/roundhouse/` through the same CI
`build-site` job that ships `/blog/`, `/browse/`, and `/bench/`. The job
assembles `_site/` (via `roundhouse::project::build_site`, which copies `site/`
wholesale + writes the `browse/` archives) and then layers on the dynamic
demos as extra steps before `upload-pages-artifact`. Two kinds of content:

- **Static** → anything under `site/` is copied as-is (this is how `/demo/`
  and the landing page ship). Links to the playground live in
  `site/index.html` and `site/demo/index.html`.
- **Generated** → a CI step writes into `_site/<dir>/`. `/blog/` emits the
  worker-profile app and vite-builds it; the **`/playground/` + `/studio/`**
  surfaces copy their HTML+entry-module dirs plus the shared `wasm/lib/`
  (driver + editor + tree + the prebuilt `roundhouse_wasm.wasm`) into
  `_site/{lib,playground,studio}/` and regenerate `fixture.json` (→ `_site/lib/`)
  from the just-built real-blog fixture (CI step "Bundle the in-browser
  playground + studio demos"). The surfaces import `../lib/`, so `_site/lib/`
  must stay a sibling of the surface dirs — the whole `_site/` tree deploys
  together (which is how Pages serves anyway).

**The playground is published with a checked-in compiler wasm — deliberately,
not blocked on #2992.** Building `roundhouse_wasm.wasm` on a runner needs the
WASI SDK *and* a `ruby-rbs-sys` reachable from CI; today that's a local-path
`[patch.crates-io]` (the wasm32 build support is upstream-pending as
ruby/rbs#2992). Rather than wait — it may be days, weeks, or never — the
~3.2 MB `wasm/lib/roundhouse_wasm.wasm` (shared by `/playground/` + `/studio/`)
is committed and the CI step just copies it. The cost is a manual refresh:
after compiler/emit changes, rebuild and recommit the binary
(`WASI_SDK_PATH=/opt/wasi-sdk cargo build --release --target wasm32-wasip1` in
`wasm/`, copy it into `wasm/lib/`, re-run `verify-playground.mjs`). See
`wasm/playground/README.md` § Maintenance.

**Switch-to-CI-build trigger:** when #2992 merges and `ruby-rbs-sys` publishes
(or the patched crate is pushed to a CI-reachable git ref), replace the copy
with a build step (install WASI SDK → `cargo build --target wasm32-wasip1` →
regenerate fixture) and delete the checked-in binary. That closes honest-gap
#1's second half. Until then, the playground is the only published surface
carrying a binary artifact in git.

The playground is pure static files (no npm/vite at serve time, no COOP/COEP),
so — like `/blog/` — it serves straight off plain GitHub Pages. One thing to
eyeball on the **first** deploy: that Pages serves the `.mjs` modules with a
JS MIME (ES-module imports are MIME-strict) and the `./roundhouse_wasm.wasm`
fetch succeeds. `transpile.mjs` uses `WebAssembly.instantiate(arrayBuffer)`
(not `instantiateStreaming`), so the wasm's own MIME is irrelevant — only that
the bytes load. If a `.mjs` MIME ever bites, rename the two driver modules to
`.js`.

## The honest last-mile gaps (read before estimating)

1. **WASI-in-browser (rung A, Phase 0). — RESOLVED.** The hand-rolled
   `wasi_snapshot_preview1` shim in `wasm/test-node.mjs` uses only
   web-platform APIs (`DataView`/`TextDecoder`/`Math.random`/`Date.now`), so
   it runs **unchanged in the browser** — no `@bjorn3/browser_wasi_shim`, no
   `wasm32-unknown-unknown`/wasm-bindgen rebuild, no npm. The spike in
   `wasm/browser-spike/` factors it into shared `wasi-shim.mjs` +
   `transpile.mjs` modules that drive both the Node validator and the page,
   and Playwright/chromium confirmed real-blog → 15 TS files (and all 6
   targets, ~5–22 ms) render in a real tab with no console errors. The
   highest-risk unknown is closed; the existing `wasm32-wasip1` artifact +
   manual C-ABI are sufficient.
   **Fresh-build gap — now mostly closed (2026-06-14).** The build needs the
   WASI SDK (`WASI_SDK_PATH=/opt/wasi-sdk`; wasi-sdk is a manual GitHub
   release tarball, not a brew formula). roundhouse already migrated off the
   `rubys/rbs-rust` fork to real `ruby-rbs = "0.3"` on all targets, and
   `WASI_SDK_PATH=/opt/wasi-sdk cargo build --release --target wasm32-wasip1`
   in `wasm/` is verified clean (3.2 MB artifact, Node round-trip OK).
   Two follow-ons remain, neither blocking Phase 1: (a) the build still leans
   on a local-path `[patch.crates-io] ruby-rbs-sys` until **ruby/rbs#2992**
   merges and publishes — repin then; (b) document `WASI_SDK_PATH` and wire
   the wasm build into CI.
2. **No editor/playground UI exists yet.** The published surfaces are
   `blog/` (running app) and `browse/` (static archive). Monaco, the file
   tree, and the output panes are all net-new.
3. **`node:test` is a Node API absent in the browser (D.2, Phase 7).** The
   emitted suites run under `node:test`/`tsx` in CI today. The runner harness
   is D.2's real work — either a shim mapping `test()/describe()/assert` onto
   a tiny browser harness, or emit to a custom runner. Everything *under* the
   harness is already browser-ready (see D.2 detail).
4. **Runtime sourcemaps are a known gap (D.2, Phase 9).** Token-level
   ERB/controller/model sourcemaps already shipped (`de99957`+`8087bef`); the
   *runtime* sourcemaps that would let a browser stack trace walk back
   through the framework runtime to the Ruby source are not done. Closing
   that gap is exactly what makes the "click the failure back to your Ruby"
   debug payoff real.

## Phase details

### Phase 0 — WASI-in-browser spike (rung A, ½–1 day)

The de-risk step. Goal: a static HTML page that loads
`roundhouse_wasm.wasm` in a browser, feeds it the real-blog fixture as the
`{src:{path→content}}` JSON, and renders the emitted TS files — no editor yet.

- Read `wasm/src/lib.rs` (done — C-ABI memory protocol documented at top of
  file) and `wasm/test-node.mjs` (the Node WASI driver to port).
- Decide the WASI strategy: **(a)** `@bjorn3/browser_wasi_shim` driving the
  existing `wasm32-wasip1` artifact unchanged, or **(b)** add a
  `wasm32-unknown-unknown` + `wasm-bindgen` build to `wasm/`. Recommend
  trying (a) first — zero compiler changes, and the artifact already exists.
- Confirm the 2.1 MB module loads and transpiles within an acceptable budget
  in a real tab (Chrome + Firefox + Safari). Note cold-load time; it informs
  whether to stream/cache the module.
- Exit criterion: real-blog → TS files rendered in a `<pre>`, in-browser.

### Phase 1 — Multi-target playground UI (rung A, 1–2 days)

- Monaco editor with a multi-file tree seeded from the real-blog fixture
  (decision #3). Editing any file updates the in-memory `src` map.
- Target dropdown: `typescript | go | rust | python | elixir | crystal` (the
  six the wasm entry point routes to today). Note in the UI that
  ruby/spinel/kotlin/swift are not yet wired into the wasm entry point — a
  one-line `match` extension in `wasm/src/lib.rs` adds any of them if wanted.
- On edit/target-change: call `transpile`, render the returned `files[]` into
  an output pane (multi-file, since output is a project). Surface
  `{error:...}` responses inline.
- Debounce transpile calls; the wasm is fast but Monaco fires often.

### Phase 3 — Diagnostics / inference overlay (rung C, 1–2 days)

The identity demo — what separates roundhouse from "yet another transpiler."

> **Status: diagnostics + inferred-type hovers SHIPPED.** `wasm/src/lib.rs`
> returns `diagnostics:[{path,start/end_line/col,severity,code,message}]` from
> `analyze::diagnose(&app)` and `inferred_types:[{…,ty}]` from a new
> `analyze::inferred_types(&app)` (a `(Span,Ty)` walker that mirrors
> `diagnose_expr`'s exhaustive recursion); `Ty`→RBS string via the now-public
> `emit::ruby::ty_to_rbs`. The playground renders diagnostics as Monaco
> squiggles + a status-bar count, and registers a Monaco hover provider that
> shows the smallest-span inferred type (RBS form) under the cursor. Editing in
> `title + 1` surfaces a live `incompatible_binop` error and a `String` hover on
> `title`. **Remaining:** only the per-file `source` provenance field.

- Extend the wasm contract: add an optional `diagnostics: [{path, line, col,
  severity, message}]` and/or `inferred_types: [{path, line, col, ty}]` to
  the `TranspileOutput` in `wasm/src/lib.rs`. The analyzer already produces
  diagnostics (thread-local emit sink + Unsupported kind, per the #28
  diagnostics work) and inferred `Ty` — this phase is plumbing them out, not
  new analysis.
- **Also add a per-file `source` field** to each emitted file (`files:
  [{path, content, source?}]`) — the source path it was generated from. The
  compiler already knows this (it's what the TS sourcemaps encode); serializing
  it for every target lets a UI map one source → its output per target. (This
  is the salvaged idea from the dropped rung B; it's also what makes "click a
  diagnostic back to the Ruby line" work across non-TS targets, so it earns its
  place here, not in a separate side-by-side view.)
- Render as Monaco markers (squiggles) + hover tooltips for inferred types.
- This dramatizes the inference-first / transpile-time-resolvable-Ruby
  positioning live; ruby2js has no type story to show.

### Phase 4 — `/studio/` scaffold + shared `lib/` + esbuild-wasm (rung D infra, 1–1½ days)

> **Status: shared `lib/` + `/studio/` scaffold DONE.** The reusable pieces are
> factored into `wasm/lib/` (`transpile.mjs` w/ new `loadDefaultCompiler` +
> `loadFixture`, `wasi-shim.mjs`, `editor.js`, the file-tree widget `tree.js`,
> plus the shared `roundhouse_wasm.wasm` binary + `fixture.json`). `/playground/`
> was refactored onto `../lib/` and re-verified green (`verify-playground.mjs`);
> the served root moved up one level (`wasm/` locally, `_site/` published) so
> `/lib/`, `/playground/`, `/studio/` sit side by side and the surfaces import
> `../lib/`. `wasm/studio/` is the new surface consuming that `lib/`: editor +
> source tree + the debounced edit→transpile loop work (TS-only), with the
> app pane showing a live build readout + the Phase 5 roadmap (it does not yet
> *run* the app). `verify-studio.mjs` smoke-checks it in chromium; the CI
> `build-site` step publishes all three dirs. **Remaining for Phase 4:**
> esbuild-wasm (below). The landing/`/demo/` links to `/studio/` are held until
> Phase 5 makes it actually run the app (avoid over-claiming).
>
> **Update — esbuild-wasm bundling DONE (Phase 4 complete).** Two pieces landed:
> (1) the wasm contract gained an optional `profile` field (`wasm/src/lib.rs`
> `TranspileInput.profile`), routing `typescript` to `emit_with_profile(worker)`
> — so `/studio/` gets the runnable SharedWorker app (86 files w/ `main.ts`+
> `worker.ts`+`src/db_worker.ts`+`vite.config.ts`), while `/playground/`'s
> default emit is unchanged (79 files). Required a wasm rebuild+recommit (3.8 MB).
> (2) `lib/bundle.mjs` bundles the emitted TS → 3 browser-loadable ESM bundles
> via **esbuild-wasm** (loaded from a CDN like Monaco — nothing vendored) with a
> virtual-FS + CDN-external plugin. Decision (below) resolved **in favor of
> esbuild**, not strip+importmap: the app is a ~50-file/~140-edge 3-entry graph,
> and workers can't use importmaps, so npm deps (`@hotwired/turbo`,
> `@sqlite.org/sqlite-wasm`) are kept as full `esm.sh` URLs (worker-safe). Studio
> bundles live on every edit (main 13KB / worker 75KB / db_worker 3KB, ~130 ms
> in chromium); `verify-studio.mjs` asserts it. The app pane shows transpile +
> bundle readouts; Phase 5 loads those bundles as the running app.

- **First step (the "shared a lot of code" part): factor the reusable pieces
  into a shared `lib/` both surfaces import** — `transpile.mjs` / `wasi-shim.mjs`
  (currently vendored in `wasm/playground/`), `editor.js`, and the source-tree +
  debounced edit→transpile loop. Then stand up `wasm/studio/` (the new
  `/studio/` surface, decision #6) consuming that `lib/`, so `/playground/`
  keeps working unchanged. `/playground/` = shared editor + code output;
  `/studio/` = shared editor + running app. Keep each a self-contained deploy
  dir (publish via a `build-site` step mirroring the playground's).
- Add `esbuild-wasm` to `/studio/`; wire a `transform`/`build` call that takes
  the emitted TS files and produces browser-loadable ESM.
- Confirm it is just-another-wasm-module — no Node, no container (decision
  #4). Measure combined load (roundhouse wasm + esbuild wasm) budget.
- Alternative to evaluate here: skip esbuild, emit strippable TS + an import
  map (the blog already uses an importmap for runtime deps). If type-stripping
  is the only need, a lighter path may exist. Pick based on what the emitted
  TS actually requires (bundling vs. bare type-strip).

### Phase 5 — Live app loop (rung D = `/studio/`, 2–3 days)

The killer demo. Reuses the blog's existing browser runtime wholesale — but as
`/studio/`'s *own* embedded app instance (its own opfs DB namespace), not the
published `/blog/`, so editing here never disturbs the standalone blog demo.

- Source of the running app: the `worker` profile TS runtime —
  `runtime/typescript/sqlite_wasm_engine.ts`, `db_worker.ts`, `juntos*.ts`
  (SharedWorker bridge), the same stack the blog ships.
- Loop: edit Ruby in Monaco → `transpile` (wasm) → esbuild bundle (Phase 4) →
  hot-swap the running app's modules → app re-renders against its sqlite-wasm
  DB. The DB persists across edits (opfs-sahpool) so state survives a
  recompile, which makes the loop feel alive.
- The hard parts are module hot-swap (re-importing changed ESM and
  re-mounting) and keeping the SharedWorker/DB alive across swaps. Study how
  the blog's vite build wires the worker; the live version replaces the
  build-time bundle with the esbuild-in-browser output.
- Exit criterion: edit a view or controller in Ruby, see the running blog
  change without a page reload.

### Phase 6 — Ship the test suite into the browser (rung D.2, ½–1 day)

- roundhouse already transpiles the Rails-style Minitest suites to TS (the
  framework-test transpile work — ValidationsTest et al. run under
  `tsx`/`node:test` in CI). This phase ensures those emitted test files are
  *included in the browser payload* alongside the app, not just built in CI.
- The fixtures path (FixtureLoader, belongs_to, the assert_select shim) is
  already proven in the emitted suites — confirm it ships to the browser too.

### Phase 7 — Browser test-runner harness (rung D.2, 1–2 days)

D.2's real work. Everything *under* the harness is already browser-ready:

- **DB**: tests want ephemeral isolation → use the sqlite-wasm **in-memory**
  fallback the engine already has (not opfs). Each run gets a fresh DB.
- **HTML assertions** (`assert_select`): *easier* in a browser — real DOM,
  no jsdom shim needed.
- **Fixtures**: FixtureLoader already works in the emitted suites.

The net-new piece is the runner: `node:test` does not exist in the browser
(gap #3). Two options:
- **(a)** A small shim exporting `test`/`describe`/`it`/`assert` that maps
  onto a browser harness and collects results. Lowest churn — the emitted
  test code stays identical to CI.
- **(b)** Emit to a custom browser runner. More control, more emit work.

Recommend (a): keep the emitted suite byte-identical to what CI runs, so the
in-browser run and the CI run are provably the same suite.

### Phase 8 — Results UI + cross-target badges (rung D.2, ½–1 day)

- A results panel: per-test green/red, counts, failure messages.
- The cross-target conformance strip: "TS ✓ (running now in your browser),
  Go ✓, Rust ✓, Python ✓ (CI-attested)". The non-TS badges are precomputed
  from CI (decision #5) — wire them from the existing per-target CI results,
  and label them as CI-attested, not live.

### Phase 9 — Runtime sourcemaps for debug (rung D.2, 1–2 days)

The "debug" leg of edit/compile/debug. A failing test should point back to
the line of **Ruby** the user wrote, not the emitted TS.

- Token-level ERB/controller/model sourcemaps already exist; this phase adds
  the *runtime* sourcemaps that let a browser stack trace walk through the
  framework runtime to the source (gap #4).
- Wire emitted sourcemaps into esbuild's output (Phase 4) so the browser's
  own stack traces resolve to Ruby; make failure locations in the results
  panel click back into Monaco at the right line.
- This is the phase that makes the cycle genuinely a *debug* cycle rather
  than a pass/fail readout.

## Risk callouts

1. **WASI-in-browser is the gating unknown.** If `@bjorn3/browser_wasi_shim`
   over the existing artifact is fiddly, the fallback (wasm32-unknown-unknown
   + wasm-bindgen) is a compiler-side change. Spike it in Phase 0 before
   committing to any UI work.
2. **Combined wasm payload size.** Measured: roundhouse wasm **3.8 MB**
   (checked-in, same-origin) + esbuild **13 MB** uncompressed (loaded from
   jsdelivr, which serves it gzip/brotli ≈ 4 MB) + sqlite-wasm (esm.sh, Phase 5).
   esbuild loads from a CDN (like Monaco), so it isn't in our repo or deploy;
   bundling after init is fast (~130 ms/edit in chromium). Acceptable for a
   demo, but the esbuild cold-load is the main spinner risk — consider caching
   or a vendored copy if CDN latency bites.
3. **Module hot-swap in rung D.** Re-importing changed ESM while keeping the
   SharedWorker + opfs DB alive is the trickiest engineering in the arc.
   De-risk by first doing a full-reload loop (recompile → reload page,
   DB persists via opfs), then optimize to hot-swap.
4. **Test isolation vs. the live app's DB.** D.2 must run tests against a
   *fresh in-memory* DB, never the live app's opfs DB, or a test run wipes
   the demo's state. Keep the two engines separate explicitly.
5. **Over-claiming live multi-target tests.** Only TS runs live. Every place
   the UI shows other targets' results, label them CI-attested. Conflating
   the two undercuts the (real, strong) conformance story with a false one.
6. **Sourcemap drift.** Three sourcemap hops (Ruby→TS via roundhouse, TS→JS
   via esbuild, JS→stack-trace in browser) must compose. If any link is
   identity-only, failures land on the wrong line. Test the composition on a
   deliberately-broken test before calling Phase 9 done.

## Mid-stream decision points

- **End of Phase 0**: WASI strategy chosen and proven. If neither shim path
  is clean, reconsider before building UI.
- **End of Phase 1**: is the multi-file editor the right model, or does a
  curated single-file-plus-hidden-app feel better for a first-time visitor?
  Decide the default-app UX before rung C piles on.
- **End of Phase 4** — RESOLVED: **esbuild-wasm**, not strip-and-importmap.
  The worker-profile app is a ~50-file/~140-edge graph across 3 entry points
  (main + SharedWorker + DB worker); bare type-strip leaves the relative graph
  unresolved, and workers can't use importmaps, so npm deps must be baked in as
  full URLs. esbuild resolves the graph in-memory and keeps the 2 npm deps as
  CDN-external full URLs (worker-safe). See `wasm/lib/bundle.mjs`.
- **End of Phase 5**: full-reload loop vs. true hot-swap — ship whichever is
  solid; hot-swap is a polish follow-on, not a gate.
- **End of Phase 7**: harness option (a) shim vs. (b) custom runner — confirm
  the in-browser suite is provably identical to CI's.

## Self-contained startup checklist (picking this up later)

1. Read this file end-to-end.
2. Read `wasm/src/lib.rs` (C-ABI + JSON contract) and `wasm/test-node.mjs`
   (the Node WASI driver to port to a browser shim).
3. Confirm the wasm artifact builds:
   `WASI_SDK_PATH=/opt/wasi-sdk cargo build --release --target wasm32-wasip1`
   in `wasm/` (the WASI SDK is required; see the rbs/libclang note above and
   gap #1). Then `node test-node.mjs` to round-trip real-blog → TS.
4. Read the blog runtime stack you'll reuse in D:
   `runtime/typescript/sqlite_wasm_engine.ts`, `db_worker.ts`, `juntos*.ts`,
   and the `build-site`/blog steps in `.github/workflows/ci.yml`
   (~lines 1228, 1307–1318) for how the worker profile is built today.
5. Read `src/bin/emit_preview.rs` (how the blog app is emitted) and the
   `worker` profile in `src/emit/typescript.rs`.
6. Start at Phase 0 — the WASI-in-browser spike is the smallest
   self-contained slice and the highest-risk unknown.

Total estimate: rung A alone **2–3 days**; full arc through the in-browser
test runner with debug sourcemaps **9–14 days**. Each rung is independently
shippable; A de-risks the wasm-in-browser path that everything else assumes.
