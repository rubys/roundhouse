# Developing Roundhouse

Day-to-day reference for working on roundhouse itself — build commands,
the `roundhouse-ast` debugging tool, how the pipeline stages compose, and
the pattern for adding a new IR variant.

## Build & test

```bash
cargo build                        # debug build
cargo build --release              # release build
cargo build --bin roundhouse-ast   # just the CLI debug tool

cargo test                         # full suite (unit + integration)
cargo test --test ingest           # one integration test file
cargo test --test real_blog        # the real-blog forcing functions
cargo test --lib erb::             # the ERB compiler unit tests
```

The test suite is the forcing function; it runs fast (sub-second) and
must pass before any commit. New IR or recognizer work lands with a
paired test — see [Adding a new IR variant](#adding-a-new-ir-variant).

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
cargo run --bin roundhouse-ast -- --round-trip -e '[:a, :b]'
cargo run --bin roundhouse-ast -- --round-trip fixtures/real-blog/app/views/articles/show.html.erb
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

### Round-trip debugging recipe

When `ir_is_fixed_under_emit_ingest` (or tiny-blog's equivalent) fails,
the `assert_eq!` panic prints both `App` structs on one line each — not
usable. Instead:

1. Narrow to one file if possible (`--round-trip PATH.erb`).
2. If the divergence is in the full-app path, use the scratch dir the
   test wrote (`target/tmp/roundhouse/real_blog_round_trip/`) and diff
   that tree against `fixtures/real-blog/`.
3. For structural (not textual) divergence, dump both `App` JSONs and
   `diff` them — the tool does this automatically for the single-file
   path.

The unified-diff output highlights exactly which IR fields flipped, and
a one-line change in the source is almost always a one-hunk change in
the JSON.

## Pipeline

```
   Ruby source  ─────────► Prism Node
                               │
                               │  ingest::ingest_expr  (src/ingest.rs)
                               ▼
   ERB source  ─►  compiled    Expr / App  (core IR, src/expr.rs + dialect)
                    Ruby          │
                    (src/erb.rs)  │  analyze::Analyzer  (src/analyze.rs)
                                  ▼
                              Expr (+ types + effects)
                                  │
                                  │  emit::{ruby, rust, go}  (src/emit/)
                                  ▼
                              emitted source code
```

Key files:

- **`src/expr.rs`** — core `Expr` / `ExprNode`. Every new language
  feature typically lands here first.
- **`src/dialect.rs`** — Rails-level structures (`Model`, `Controller`,
  `Action`, `View`, `RouteTable`, …).
- **`src/ingest.rs`** — Prism → IR. One match arm per `ExprNode` kind.
  Unknown constructs return `IngestError::Unsupported` — loud by design.
- **`src/erb.rs`** — ERB → Ruby source string. Output is the input to
  the regular Ruby ingest path.
- **`src/analyze.rs`** — type inference + effect inference. Two walks
  (`compute` for types, `visit_effects` for effects).
- **`src/emit/`** — one file per target. Ruby emit pairs with ingest as
  the round-trip identity forcing function. Targets today:
  `ruby` (full), `rust` / `go` / `typescript` / `elixir` (scaffolds — Phase 2).

## Fixtures

`fixtures/tiny-blog` is the minimal always-works fixture. Its tests
(source-equivalence + round-trip-identity) gate every commit.

`fixtures/real-blog` is the Phase 1 target — a modernized Rails 8 blog
from ruby2js's `demo-blog.tar.gz`, checked in verbatim (including
`test/`). Three tests in `tests/real_blog.rs` pair against it:

1. `ingests_without_errors` — loud regression guard.
2. `expected_files_round_trip_byte_for_byte` — inclusion list; promote
   files as their remaining gaps close.
3. `ir_is_fixed_under_emit_ingest` — structural round-trip across the
   whole app (what `roundhouse-ast --round-trip` does for one file).

Known gaps are in `fixtures/real-blog/README.md`, in priority order.
The priority is set by "what breaks next as the probe advances."

## Adding a new IR variant

The pattern today (example: adding `ExprNode::Array`):

1. **Declare the variant.** Add to `src/expr.rs`. Include any surface-
   preservation fields needed for byte-for-byte round-trip (e.g.
   `ArrayStyle` for `[:a]` vs `%i[a]` vs `%w[a]`).

2. **Ingest.** Add an arm to `ingest::ingest_expr` in `src/ingest.rs`
   matching the relevant `as_*_node()`. Extract surface-preservation
   fields from location bytes when needed.

3. **Analyze.** Add cases in both `analyze::Analyzer::compute` (the
   type walk) and `visit_effects` (the effect walk). Omit either at
   your peril — missing effect propagation is a silent bug.

4. **Emit.** Add a match arm in each of `src/emit/ruby.rs`,
   `src/emit/rust.rs`, `src/emit/go.rs`. Ruby's arm must be the exact
   inverse of the ingest (that's the round-trip forcing function);
   Rust/Go can be approximations until a target fixture sharpens them.

5. **Test.** Add a unit test to `tests/ingest.rs` (one `parse_one`
   helper call per surface form you claim to preserve). Run
   `cargo test --test ingest`. If the new variant appears in
   tiny-blog, the source-equivalence + round-trip tests will also
   catch regressions automatically.

6. **Verify.** `cargo run --bin roundhouse-ast -- --round-trip -e 'EXAMPLE'`
   should print `ok: IR stable across …`.

**Common traps:**

- Forgetting to add the match arm in `visit_effects` — code compiles
  (it's a catch-all match), effects silently don't propagate.
- Normalizing source detail at ingest (e.g. stripping `%i[…]` style)
  breaks source-equivalence even though round-trip-identity passes.
  Keep a distinct IR field for anything that would diverge on emit.
- Emit-side parens: `emit_send_base` respects `parenthesized` for both
  implicit-self and explicit-receiver calls — don't regress this.
- Adjacent text chunks in ERB must stay merged across comment tags;
  `compile_erb` buffers `pending_text` and only flushes on meaningful
  tags. Bypass at your peril.

## See also

- **`fixtures/real-blog/README.md`** — remaining ingest gaps with
  priority order.
- **`fixtures/tiny-blog/`** — the minimal working fixture.
- **Auto-memory** (`~/.claude/projects/-Users-rubys-git-railcar/memory/`)
  — strategy notes, target roadmap, ERB fidelity plan, comment
  preservation plan.
