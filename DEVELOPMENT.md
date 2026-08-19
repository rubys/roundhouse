# Developing Roundhouse

Day-to-day reference for working on roundhouse itself — build commands,
the `roundhouse-ast` debugging tool, how the pipeline stages compose, and
the pattern for adding a new IR variant.

For deeper architecture and per-stage internals, see [`docs/`](docs/).

## Build & test

```bash
cargo build                        # debug build
cargo build --release              # release build
cargo build --bin roundhouse-ast   # just the CLI debug tool

cargo test                         # full default suite (unit + integration)
cargo test --test ingest           # one integration test file
cargo test --test real_blog        # the real-blog forcing functions
cargo test --lib erb::             # the ERB compiler unit tests

cargo test --test rust_toolchain -- --ignored       # real Rust build
cargo test --test typescript_toolchain -- --ignored # real TS build
# ...one <target>_toolchain per target (crystal, go, elixir, python,
# kotlin, swift, csharp, ruby, spinel, roda)

cargo test --test framework_tests_ruby -- --ignored # framework runtime's
# own test suite (runtime/ruby/test/) run against a target's transpile;
# framework_tests_{crystal,kotlin,rust,spinel,swift,typescript} likewise
```

The default test suite is the forcing function and must pass before
any commit. Toolchain and framework tests are `#[ignore]`-gated so a
local `cargo test` doesn't require every target runtime installed —
CI covers them via per-target jobs and the `smoke` matrix (which
executes each published archive's README verbatim; several toolchain
jobs were retired in its favor and remain dev-loop harnesses — see
the comment in `.github/workflows/ci.yml`).

New IR or recognizer work lands with a paired test — see [Adding a new
IR variant](#adding-a-new-ir-variant).

## Fixtures

### tiny-blog

`fixtures/tiny-blog/` is the minimal always-works fixture. Its gates
run in the default suite: zero diagnostics (`tests/analyze.rs`) and
IR-shape assertions (`tests/ingest.rs`), plus per-target toolchain
gates for the absent-feature shape real-blog can't test. Checked
into the repo — safe to edit directly when extending coverage.

### real-blog

`fixtures/real-blog/` is the Phase-1 target — a modernized Rails 8
blog. It is **not checked in**; it's derived on demand from
`scripts/create-blog` (a frozen snapshot of the ruby2js upstream
generator, reproducible without an external git checkout).

```bash
bin/rh fixture          # regenerate into fixtures/real-blog/
bin/rh clean fixture    # remove it
```

CI regenerates the fixture once per run in the `generate-fixture` job
and shares the artifact across the unit job and every per-target
job — see [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

`tests/real_blog.rs` pairs against the generated tree; its
load-bearing gates:

1. `ingests_without_errors` / `ingests_without_parse_diagnostics` —
   loud regression guards.
2. `type_analysis_coverage` — the contract test: zero error
   diagnostics and zero unresolved types across the whole fixture.
3. `model_tests_ingest_into_test_modules` / `fixtures_ingest_into_app`
   — the test/fixture ingest surface.

The emit-side forcing functions live in `tests/lowered_ruby_emit.rs`
and `tests/spinel_toolchain.rs` (whole-app source-equivalence
round-trip was retired in favor of compile-equivalence via Spinel —
see the header of `src/emit/ruby.rs`).

## Debugging tools

### `roundhouse-ast`

The pipeline has four distinct stages (Prism parse → ERB compile → ingest
→ emit), and debugging almost always means "show me what stage N produced
for this input." Rust's `{:?}` Debug output isn't readable for our IR
at scale, and shelling out to `ruby --dump=parsetree` only shows Prism's
view. `roundhouse-ast` is the structural-dump tool.

Run it via `cargo run --bin roundhouse-ast --` or build once and invoke
`target/debug/roundhouse-ast` directly.

**Quick examples:**

```bash
# Default: ingest a Ruby snippet, show the IR as JSON
cargo run --bin roundhouse-ast -- -e '[:a, :b]'

# See what Prism produced before our ingest ran
cargo run --bin roundhouse-ast -- --stage prism -e '@x.y do end'

# See the ERB compiler's Ruby output
cargo run --bin roundhouse-ast -- --stage compile-erb view.html.erb

# Emit Ruby back from one expression's IR
cargo run --bin roundhouse-ast -- --stage emit-ruby -e '"a#{x}b"'

# Run every stage in pipeline order, with headers
cargo run --bin roundhouse-ast -- --stages --erb -e '<%= x %>'

# End-to-end round-trip: ingest → emit → ingest, IR-diff on divergence
# (Ruby input only — ERB round-trip retired with the parsed-AST emitter)
cargo run --bin roundhouse-ast -- --round-trip -e '[:a, :b]'
cargo run --bin roundhouse-ast -- --round-trip fixtures/tiny-blog/app/models/post.rb
```

**Flag reference:**

| Flag | Purpose |
|------|---------|
| `-e CODE` | Inline Ruby source |
| `PATH` | Positional file (`.rb` or `.erb`; extension chooses ERB mode) |
| `--erb` | Force ERB compilation on inline input |
| `--stage NAME` | `prism`, `compile-erb`, `ingest` (default), `emit-ruby` |
| `--stages` | Run every stage, print each with a header |
| `--round-trip` | Ingest → emit → re-ingest; exit non-zero if IR diverges |
| `-h`, `--help` | Print usage |

The JSON output for IR stages uses `serde_json::to_string_pretty` on the
`Expr` type — one field per line, deterministic key ordering — which is
also why structural diffs fall out naturally when two IRs disagree.

### `dump_ir`

Dump the *lowered* IR for a fixture — what the emitters actually
consume, after analyze and the post-analyze passes. Takes a
`Class#method` selector to narrow output. When an emitter produces
wrong code, this is the first question: is the IR wrong, or the
emitter's walk of it?

```bash
cargo run --bin dump_ir -- --help
```

### `emit_preview`

Emit one target's tree from a fixture straight to disk (default
`/tmp/rh-<target>-pass2`, override with `--out`) — the fastest way to
eyeball what a change did to emitted output without `--site`
packaging. See the header of `src/bin/emit_preview.rs`.

### `roundhouse-compare`

Cross-runtime HTML equivalence check. Boot Rails on one port, boot a
roundhouse-emitted runtime on another, hand `roundhouse-compare` a URL
list, and it walks the canonicalized DOM trees side-by-side looking for
the first structural divergence. Lives in `tools/compare/` (a
standalone crate — build it there, or use `scripts/compare` which
drives the whole flow). See
[`docs/pipeline/verification.md`](docs/pipeline/verification.md).

### Round-trip debugging recipe

When `roundhouse-ast --round-trip` fails on a file or expression, it
dumps both IR JSONs and diffs them automatically — the unified diff
highlights exactly which IR fields flipped, and a one-line change in
the source is almost always a one-hunk change in the JSON. For
emit-side divergence (the lowered-IR gates in
`tests/lowered_ruby_emit.rs`), pair the failing assertion with
`dump_ir` on the same selector to see the IR the emitter was handed.

## Pipeline at a glance

```
   Ruby source  ─────────► Prism Node
                               │
                               │  ingest::ingest_expr  (src/ingest/expr.rs)
                               ▼
   ERB / HAML  ─►  compiled    Expr / App  (core IR, src/expr.rs + dialect)
                    Ruby          │
                    (src/erb.rs,  │  analyze::Analyzer  (src/analyze/)
                     src/haml.rs) ▼
                              Expr (+ types + effects)
                                  │
                                  │  lower::apply_post_analyze_lowerings
                                  │  + the *_to_library passes  (src/lower/)
                                  ▼
                              LibraryClass / LibraryFunction / FlatRoute / ...
                                  │
                                  │  emit::{ruby, rust, typescript, ...}
                                  │  dispatched by src/project.rs::target_files
                                  ▼
                              emitted source code  +  runtime/<target>/ glue
```

Key files:

- **`src/expr.rs`** — core `Expr` / `ExprNode`. Every new language
  feature typically lands here first.
- **`src/dialect.rs`** — Rails-level structures (`Model`, `Controller`,
  `View`, `RouteTable`, …) and the lowered `LibraryClass` /
  `LibraryFunction` contract emitters consume.
- **`src/ingest/`** — Prism → IR. `expr.rs` holds `ingest_expr`, one
  match arm per node kind; per-concern modules (model, controller,
  routes, view, …) sit alongside — `mod.rs` is the roster. Unknown
  constructs return `IngestError::Unsupported` in strict mode; survey
  mode records and continues (`src/ingest/survey.rs`).
- **`src/erb.rs` / `src/haml.rs`** — template → Ruby source string.
  Output is the input to the regular Ruby ingest path
  (`src/ingest/view.rs` is the engine seam).
- **`src/analyze/`** — type inference + effect inference. The type
  walk is `BodyTyper` in `src/analyze/body/mod.rs`; the effect walk is
  in `src/analyze/effects.rs`; `mod.rs` orchestrates the fixpoint. See
  [`docs/pipeline/analyze.md`](docs/pipeline/analyze.md).
- **`src/catalog/`** — method catalog; single source of truth for the
  AR method surface (plus the gem catalog in `gems.rs`). See
  [`docs/data/catalog.md`](docs/data/catalog.md).
- **`src/adapter.rs`** — `DatabaseAdapter` trait. See
  [`docs/data/adapter.md`](docs/data/adapter.md).
- **`src/lower/`** — target-neutral lowerings.
  `POST_ANALYZE_PASS_ORDER` in `src/lower/mod.rs` is the ordering
  authority for the post-analyze pass pipeline; the `*_to_library`
  modules produce the lowered shapes. See
  [`docs/pipeline/lower.md`](docs/pipeline/lower.md).
- **`src/emit/`** — one module per target (`<target>.rs` +
  `<target>/` submodules). Dispatch lives in
  `src/project.rs::target_files`. See
  [`docs/pipeline/emit.md`](docs/pipeline/emit.md).
- **`src/runtime_loader.rs`** — transpiles `runtime/ruby/` (the
  framework runtime) into each target at emit time.
- **`runtime/<target>/`** — hand-written per-target glue copied
  verbatim into emitted projects. See
  [`docs/pipeline/runtime.md`](docs/pipeline/runtime.md).

## Adding a new IR variant

The pattern today (example: adding `ExprNode::Array`):

1. **Declare the variant.** Add to `src/expr.rs`. Include any surface-
   preservation fields needed for byte-for-byte round-trip (e.g.
   `ArrayStyle` for `[:a]` vs `%i[a]` vs `%w[a]`).

2. **Ingest.** Add an arm to `ingest_expr` in `src/ingest/expr.rs`
   matching the relevant `as_*_node()`. Extract surface-preservation
   fields from location bytes when needed.

3. **Analyze.** Add cases in both `BodyTyper::compute`
   (`src/analyze/body/mod.rs`, the type walk) and `visit_effects`
   (`src/analyze/effects.rs`, the effect walk). Omit either at
   your peril — missing effect propagation is a silent bug.

4. **Emit.** Add a match arm in each live emitter's expression module
   (`src/emit/ruby/expr.rs`, `src/emit/typescript/expr.rs`,
   `src/emit/rust/expr/`, …). Ruby's arm must invert the ingest —
   that's what `roundhouse-ast --round-trip` checks; other targets can
   be approximations until a fixture sharpens them.

5. **Test.** Add a unit test to `tests/ingest.rs` (one `parse_one`
   helper call per surface form you claim to preserve). Run
   `cargo test --test ingest`. If the new variant appears in
   tiny-blog, its zero-diagnostics and IR-shape gates will also
   catch regressions automatically.

6. **Verify.** `cargo run --bin roundhouse-ast -- --round-trip -e 'EXAMPLE'`
   should print `ok: IR stable across …`.

**Common traps:**

- Forgetting to add the match arm in `visit_effects` — code compiles
  (it's a catch-all match), effects silently don't propagate.
- Normalizing source detail at ingest (e.g. stripping `%i[…]` style)
  silently rewrites the author's spelling on emit. Keep a distinct IR
  field for anything that would diverge — that's what the
  surface-preservation fields on `Expr` exist for.
- Emit-side parens: `emit_send_base` respects `parenthesized` for both
  implicit-self and explicit-receiver calls — don't regress this.
- Adjacent text chunks in ERB must stay merged across comment tags;
  `compile_erb` buffers `pending_text` and only flushes on meaningful
  tags. Bypass at your peril.

## Repo map

Beyond `src/` and `tests/`, the directories a newcomer will meet:

- **`src/bin/`** — seven binaries: `roundhouse` (the main CLI:
  `--target` / `--site`), `roundhouse-ast`, `roundhouse-check`,
  `roundhouse-lsp` and `roundhouse-mcp` (the inference engine's LSP
  and MCP servers), `dump_ir`, `emit_preview`.
- **`runtime/`** — per-target primitive runtimes plus `runtime/ruby/`,
  the framework runtime transpiled into every target (and its own
  test suite under `runtime/ruby/test/`).
- **`fixtures/`** — `tiny-blog/` (checked in), `real-blog/`
  (generated; see above), `roda-blog/` (the experimental Roda target's
  fixture).
- **`tools/compare/`** — standalone crate: the `roundhouse-compare`
  DOM/JSON differ.
- **`scripts/`** — workflow scripts; the load-bearing ones are
  `create-blog`, `compare`, `bench`, `smoke` (executes published
  archives' READMEs verbatim — a CI contract), `e2e`, and the
  `campfire-*` / `lobsters-*` conformance harnesses.
- **`e2e/`** — Playwright specs for the dynamic behavior a static DOM
  diff can't reach (see `e2e/README.md`); `tests/browser_smoke/` is
  the browser-side smoke harness.
- **`wasm/`** — a second crate: the compiler built for WebAssembly,
  plus the in-browser playground / IDE / studio surfaces.
- **`editors/vscode/`** — VS Code extension wrapping `roundhouse-lsp`.
- **`kotlin-reference/` / `swift-reference/`** — hand-written
  reference apps that served as forcing functions for those emitters
  (see their READMEs).
- **`site/`** — static assets for the GitHub Pages site; `bench/` —
  benchmark harness inputs.

## See also

- [`docs/README.md`](docs/README.md) — index of all architecture docs
  and working plans.
- [`docs/data/`](docs/data/) — the compiler's inputs (Ruby + ERB,
  schema/routes/seeds, method catalog, database adapter).
- [`docs/pipeline/`](docs/pipeline/) — analyze, lower, emit, runtime
  integration, verification.
- [`AGENTS.md`](AGENTS.md) — orientation and invariants for agents and
  new contributors.
