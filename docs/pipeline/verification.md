# Verification

How roundhouse knows its output is correct. Four layers, each with a
different failure mode to catch.

## The four layers

| Layer | What it catches | Where |
|-------|-----------------|-------|
| **Source equivalence** | Ingest silently dropped a construct | `tests/source_equivalence.rs` |
| **Round-trip identity** | IR is lossy — emit dropped information the ingester produced | `tests/round_trip_identity.rs`, `tests/real_blog.rs`, `--round-trip` CLI |
| **Toolchain compile** | Emitted project doesn't build / type-check in its target language | `tests/<target>_toolchain.rs` (`--ignored`) |
| **Cross-runtime DOM** | Emitted runtime renders HTML that diverges from Rails | `tools/compare/` |

Each layer has a different blind spot the next layer catches.

## Source equivalence

**Claim:** emitted Ruby equals the fixture source byte-for-byte.

**File:** `tests/source_equivalence.rs` (gates against
`fixtures/tiny-blog/`).

**Why it exists:** if the ingester silently drops a construct, the
emitter has nothing to emit, a round-trip test ingests nothing both
times and trivially passes. Source equivalence closes that hole — the
diff shows exactly which file drifted.

**Failure recipe:** almost always an ingest gap (a Rails construct the
recognizer doesn't handle yet). Add the recognizer, don't relax the
test.

## Round-trip identity

**Claim:** `ingest(source) → emit_ruby → ingest → App` ≡ the original
ingest's `App`.

**Files:**
- `tests/round_trip_identity.rs` — tiny-blog.
- `tests/real_blog.rs::ir_is_fixed_under_emit_ingest` — whole real-blog.
- `tests/real_blog.rs::expected_files_round_trip_byte_for_byte` — the
  inclusion list; real-blog files promoted as gaps close.
- `cargo run --bin roundhouse-ast -- --round-trip PATH.erb` — interactive,
  one file at a time.

**What it catches:** IR holes. If the emitter produces a form the
ingester doesn't accept, or the IR dropped information the emitter
needed, the second ingest produces a different `App` and `assert_eq!`
panics.

**Debugging failures:** the panic prints both `App` structs on one
line each (not useful at scale). Instead:

1. Narrow to one file: `roundhouse-ast --round-trip PATH.erb`.
2. If divergence is in the full-app path, use the scratch dir
   (`CARGO_TARGET_TMPDIR/roundhouse/real_blog_round_trip/`) and diff
   that tree against `fixtures/real-blog/`.
3. For structural divergence, dump both `App` JSONs via
   `roundhouse-ast --stage ingest` and diff.

The JSON's deterministic key ordering makes diffs trivially
localizable — a one-line change in source is almost always a one-hunk
change in the IR.

## Toolchain compile

**Claim:** emitted projects build with the real target toolchain.

**Files:** `tests/rust_toolchain.rs`, `tests/typescript_toolchain.rs`,
`tests/go_toolchain.rs`, `tests/crystal_toolchain.rs`,
`tests/elixir_toolchain.rs`, `tests/python_toolchain.rs`. All marked
`#[ignore]` so `cargo test` doesn't require six language toolchains
installed — CI runs each in its own job. Invoke locally:

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

1. **`generate-fixture`** — runs `make real-blog` once and packs the
   result as an artifact.
2. **`unit`** — downloads the artifact and runs `cargo test
   --all-targets` (source equivalence + round-trip identity +
   analyzer + ingest + emit snapshots).
3. **`toolchain-<target>`** — one job per target. Each installs the
   target toolchain, downloads the fixture, and runs
   `cargo test --test <target>_toolchain -- --ignored`.
4. **`build-site`** — gated on every toolchain job succeeding. Builds
   the Pages manifest only when every target compiles its emitted
   project cleanly.
5. **`deploy`** — pushes the Pages artifact on `main` only.

Fixture generation happens once per CI run; every subsequent job
reuses it. `ruby/setup-ruby` + `rails` install costs are paid once,
not six times.

The cross-runtime DOM check isn't in CI yet — it needs two running
servers per target, which is more orchestration than the current
setup provides. Running it locally before a merge that touches the
emit/runtime boundary is the current convention.

## Key files

| File | Role |
|------|------|
| `tests/source_equivalence.rs` | tiny-blog byte-for-byte |
| `tests/round_trip_identity.rs` | tiny-blog IR stability |
| `tests/real_blog.rs` | real-blog forcing functions |
| `tests/<target>_toolchain.rs` | `--ignored` real-toolchain builds |
| `tools/compare/` | Cross-runtime DOM comparator |
| `src/bin/roundhouse-ast.rs` | Interactive pipeline inspector |
| `.github/workflows/ci.yml` | CI layout |

## Related docs

- [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md) — round-trip
  debugging recipe.
- [`emit.md`](emit.md) — what the toolchain tests validate.
- [`runtime.md`](runtime.md) — what the DOM comparator validates.
