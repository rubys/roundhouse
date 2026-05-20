# Verification

How roundhouse knows its output is correct. Four layers, each with a
different failure mode to catch.

## The four layers

| Layer | What it catches | Where |
|-------|-----------------|-------|
| **Ingest coverage** | Ingest silently dropped or mis-typed a construct | `tests/real_blog.rs`, `tests/ingest.rs` |
| **Round-trip identity** | IR is lossy — emit dropped information the ingester produced | `tests/roundtrip.rs`, `tests/runtime_src_roundtrip.rs`, `--round-trip` CLI |
| **Toolchain compile** | Emitted project doesn't build / type-check in its target language | `tests/<target>_toolchain.rs` (`--ignored`) |
| **Cross-runtime DOM** | Emitted runtime renders HTML that diverges from Rails | `tools/compare/` |

Each layer has a different blind spot the next layer catches.

## Ingest coverage

**Claim:** the ingester loads `fixtures/real-blog/` cleanly, the
analyzer types every expression, and zero error diagnostics fire.

**Files:**
- `tests/real_blog.rs::ingests_without_errors` — loud-by-design
  regression guard against any unsupported construct.
- `tests/real_blog.rs::model_tests_ingest_into_test_modules`,
  `fixtures_ingest_into_app` — domain-specific coverage.
- `tests/real_blog.rs::type_analysis_coverage` — the zero-error-
  diagnostics gate; the subset of programs roundhouse can transpile.
- `tests/ingest.rs` — finer-grained construct-level coverage.

**Why it exists:** if the ingester silently drops a construct, the
emitter has nothing to emit, and downstream tests trivially pass. The
ingest-coverage gate closes that hole — `Unsupported` errors are
loud, and the analyzer's diagnostic pipeline catches anything that
ingested but didn't type.

**Failure recipe:** almost always an ingest gap (a Rails construct
the recognizer doesn't handle yet) or an analyzer gap. Add the
recognizer, don't relax the test.

## Round-trip identity

**Claim:** `ingest(source) → emit_ruby → ingest → App` ≡ the original
ingest's `App`. Also: `App → JSON → App` is identity.

**Files:**
- `tests/roundtrip.rs::tiny_blog_round_trips` — hand-constructed `App`
  through `serde_json::{to_string, from_str}`. The forcing function for
  IR completeness.
- `tests/roundtrip.rs::literals_round_trip` — every literal kind.
- `tests/runtime_src_roundtrip.rs` — framework-Ruby `MethodDef` →
  emit → re-ingest stability.
- `cargo run --bin roundhouse-ast -- --round-trip -e '<ruby>'` or
  `--round-trip PATH.rb` — interactive, one snippet at a time.
  (ERB round-trip was removed when the spinel emit pipeline dropped
  the parsed-AST emitter — the tool errors on `.erb` inputs.)

**What it catches:** IR holes. If the emitter produces a form the
ingester doesn't accept, or the IR dropped information the emitter
needed, the second ingest produces a different `App` and `assert_eq!`
panics.

**Debugging failures:** the panic prints both `App` structs on one
line each (not useful at scale). Instead:

1. Narrow to one snippet: `roundhouse-ast --round-trip -e '<expr>'`
   or `--round-trip PATH.rb`.
2. For structural divergence, dump both `App` JSONs via
   `roundhouse-ast --stage ingest` and diff.

The JSON's deterministic key ordering makes diffs trivially
localizable — a one-line change in source is almost always a one-hunk
change in the IR.

## Toolchain compile

**Claim:** emitted projects build with the real target toolchain.

**Files:** `tests/rust_toolchain.rs`, `tests/typescript_toolchain.rs`,
`tests/go_toolchain.rs`, `tests/crystal_toolchain.rs`,
`tests/elixir_toolchain.rs`, `tests/python_toolchain.rs`,
`tests/ruby_toolchain.rs`. All marked `#[ignore]` so `cargo test`
doesn't require every language toolchain installed — CI runs each in
its own job. Invoke locally:

```bash
cargo test --test rust_toolchain -- --ignored --nocapture
```

**What each test does:** generate the target project in a scratch
dir, invoke `cargo build` / `tsc --noEmit` / `go build` / `mix
compile` / `crystal build` / `python -m py_compile`, assert success.
For Rust, `tests/emit_rust.rs::boots_real_blog_server` goes further —
it starts the emitted binary, issues HTTP requests, and checks
responses.

**What it catches:** emitted code that's well-formed IR-side but
rejected by the target toolchain — wrong `use` statements, missing
imports, type-checker complaints, borrow-checker issues.

## Cross-runtime DOM comparison

**Claim:** the HTML emitted by a roundhouse-generated runtime is DOM-
equivalent to the HTML Rails emits for the same request.

**Tool:** `tools/compare/` (the `roundhouse-compare` binary).

**Contract** (per the tool's module doc): "same DOM when inspected by
JS or CSS". That means:

- Tag tree must match exactly (same elements, same children).
- Text nodes match byte-for-byte (whitespace included).
- Attribute order is insignificant (canonicalized to sorted).
- HTML comments are insignificant (dropped during canon).
- Specific known-variable values (CSRF tokens, asset fingerprints,
  session ids) get replaced with placeholders per a YAML ignore-rules
  config.

**How to run:**

```bash
# In one terminal: boot Rails against the fixture
cd fixtures/real-blog && bin/rails server -p 4000

# In another: boot the emitted Rust project
# (built by tests/rust_toolchain.rs; path printed on success)
cd /tmp/.../emitted-rust && cargo run --release -- --port 4001

# In a third: run the comparator
cargo run --bin roundhouse-compare -- \
    --reference http://localhost:4000 \
    --target http://localhost:4001 \
    --urls urls.txt \
    --config tools/compare/config.example.yaml
```

**Ignore rules** (`tools/compare/config.example.yaml`):

- Drop `<meta name="csrf-token">`, `<meta name="csp-nonce">`,
  `<meta name="csrf-param">`.
- Blank `authenticity_token` hidden form fields.
- Strip `?v=...` query strings from stylesheet/script URLs (Propshaft
  fingerprints).
- Strip the signature suffix from `<turbo-cable-stream-source
  signed-stream-name="...">` — the base64'd channel name before `--`
  must match; the HMAC after it differs.

**Failure recipe:** a genuine divergence is a bug — either in the
emitter's lowering, or in a view helper's output shape. Extend the
ignore-rules only for values that are structurally meaningful but
legitimately per-request (CSRF tokens, session ids, asset
fingerprints).

## `roundhouse-ast` — the interactive debugger

Not a test, but the tool you'll reach for when a test fails. It
exposes every pipeline stage as a dump:

```bash
# Default: ingest, print the IR as JSON
roundhouse-ast -e '[:a, :b]'

# See what Prism produced before ingest ran
roundhouse-ast --stage prism -e '@x.y do end'

# See the ERB compiler's Ruby output
roundhouse-ast --stage compile-erb view.html.erb

# Emit Ruby from an IR
roundhouse-ast --stage emit-ruby -e '"a#{x}b"'

# Every stage in order, with headers
roundhouse-ast --stages --erb -e '<%= x %>'

# Round-trip check on one file
roundhouse-ast --round-trip fixtures/real-blog/app/views/articles/show.html.erb
```

`--stage ingest` uses `serde_json::to_string_pretty`, so structural
diffs between two IRs drop out from plain `diff` across the two
outputs.

## CI layout

`.github/workflows/ci.yml` is structured around the same four
layers:

1. **`generate-fixture`** — runs `bin/rh fixture` once and packs the
   result as an artifact.
2. **`unit`** — downloads the artifact and runs `cargo test
   --all-targets` (round-trip identity + analyzer + ingest + emit
   snapshots).
3. **`toolchain-<target>`** — one job per target. Each installs the
   target toolchain, downloads the fixture, and runs
   `cargo test --test <target>_toolchain -- --ignored`.
4. **`compare-<target>`** — one job per target (rust, crystal,
   typescript, go, elixir, python, ruby). Each runs
   `scripts/compare <target>`, which boots Rails on the fixture and
   the emitted runtime, then DOM-diffs the responses for a fixed URL
   set. The Pages deploy is gated on every compare job passing — any
   DOM drift fails the build.
5. **`build-site`** — gated on every toolchain + compare job
   succeeding. Builds the Pages manifest.
6. **`deploy`** — pushes the Pages artifact on `main` only.

Fixture generation happens once per CI run; every subsequent job
reuses it. `ruby/setup-ruby` + `rails` install costs are paid once,
not seven times.

`compare-spinel` is the open future job — `scripts/compare ruby`
covers DOM parity for the lowered Ruby today via shell-out to
`main.rb`; the spinel-compiled binary plugs into the same compare
harness once end-to-end runnable.

## Key files

| File | Role |
|------|------|
| `tests/real_blog.rs` | Ingest + analyzer coverage on the real-blog fixture |
| `tests/roundtrip.rs` | IR → JSON → IR identity (hand-constructed `App` + literal coverage) |
| `tests/runtime_src_roundtrip.rs` | Framework-Ruby round-trip identity |
| `tests/ingest.rs` | Construct-level ingest coverage |
| `tests/<target>_toolchain.rs` | `--ignored` real-toolchain builds (rust, ts, go, crystal, elixir, python, ruby) |
| `tools/compare/` | Cross-runtime DOM comparator |
| `scripts/compare` | CI driver for `compare-<target>` jobs |
| `src/bin/roundhouse-ast.rs` | Interactive pipeline inspector |
| `.github/workflows/ci.yml` | CI layout |

## Related docs

- [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md) — round-trip
  debugging recipe.
- [`emit.md`](emit.md) — what the toolchain tests validate.
- [`runtime.md`](runtime.md) — what the DOM comparator validates.
