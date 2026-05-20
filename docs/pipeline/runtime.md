# Runtime

Each emitted project links against a per-target runtime that ships
in two layers: **target primitives** (hand-written) and the
**framework runtime** (transpiled from Ruby).

**Source:** `runtime/<target>/` for primitives; `runtime/ruby/` for
the framework runtime (consumed by all non-Ruby targets via the same
roundhouse pipeline that compiles user apps).

## The two-layer split

```
runtime/ruby/                    ← single source of framework Ruby
  active_record/                    transpiled into the emit pipeline
  action_controller/                  ↓
  action_view/                      framework-runtime files appear in
  action_dispatch/                  the emitted project (e.g. TS emit
  inflector.rb                      writes src/active_record_base.ts,
  json_builder.rb                   src/action_controller_base.ts, …)
  ...

runtime/<target>/                ← per-target primitives, hand-written
  db.<ext>                          DB connections, HTTP server,
  server.<ext>                      WebSocket plumbing, test harness —
  cable.<ext>                       genuinely target-idiomatic glue
  test_support.<ext>                that no IR-level lowering captures
  ...
```

### Why two layers?

- **Framework runtime is target-uniform Rails surface.** Models'
  `validates`, controller action helpers, view helpers like
  `link_to` / `form_with` / `pluralize` — these have one canonical
  Ruby implementation. Authoring them N times in N target
  languages is exactly the duplication the lowering layer was
  built to eliminate; transpiling them once subsumes it.
- **Target primitives are unavoidably idiomatic.** Wiring axum
  middleware vs. Node's `http` module vs. Plug's pipeline DSL vs.
  Crystal's `HTTP::Server` looks different in a way no IR
  captures. Hand-writing these stays cheap because each target's
  primitive layer is small (a few hundred lines).

The forcing function: **any target that compiles user Rails apps
must also compile `runtime/ruby/`**. If your emitter can transpile
ApplicationRecord + FormBuilder + Inflector, it can transpile a
real Rails app. Phase 1 of the unified-IR plan front-loaded this
risk by transpiling `runtime/ruby/` to TypeScript before any user-
app emission depended on the result.

## Target primitives — what's in each `runtime/<target>/`

Conventional file roles (file names vary by target — see inventory below):

| Role | Description |
|------|-------------|
| Model base / shims | `ActiveRecordAdapter` trait, validation error type, framework-test adapter |
| DB connection | Lifecycle (open, with_conn borrow, test-mode in-memory) |
| HTTP server | Production HTTP entry — listens on a port, dispatches through Router |
| Action Cable | WebSocket endpoint |
| View helpers | Delegates into transpiled framework Ruby (where present) or implements helpers directly (legacy) |
| Test support | TestClient + TestResponse + Rails-shaped assertions |

The shape varies per target — newer targets (Rust, Crystal) carry
adapter + framework-test scaffolding the older ones don't yet. The
current inventory:

| Target | Files |
|--------|-------|
| `runtime/rust/` | `active_record_adapter.rs`, `cable.rs`, `db.rs`, `errors_ext.rs`, `flash.rs`, `framework_test_adapter.rs`, `hash_ext.rs`, `http.rs`, `param_value.rs`, `runtime.rs`, `server.rs`, `session.rs`, `test_support.rs`, `view_helpers.rs` |
| `runtime/crystal/` | `broadcasts.cr`, `cable.cr`, `db.cr`, `framework_test_adapter.cr`, `http.cr`, `param_value.cr`, `server.cr`, `test_helper.cr`, `test_support.cr` |
| `runtime/typescript/` | DB / server / Cable primitives (`db.ts`, `db-libsql.ts`, `db_worker.ts`, `server.ts`, `server-libsql.ts`, `server-worker.ts`, `broadcasts.ts`, `client.ts`, `param_value.ts`, `sqlite_wasm_engine.ts`), the worker bridge (`juntos*.ts`), and async/sync minitest adapters (`minitest.ts`, `minitest-async.ts`). Framework-runtime files (`active_record_base.ts`, `action_controller_base.ts`, `inflector.ts`, `json_builder.ts`, …) are emitter-generated from `runtime/ruby/` and appear under `src/` in emitted projects, not in this directory. |
| `runtime/go/`, `runtime/python/`, `runtime/elixir/` | Conventional 7-file primitive set (`cable`, `db`, `http`, `runtime`, `server`, `test_support`, `view_helpers`) |
| `runtime/spinel/` | Per-target primitives for the Ruby/Spinel target (`base64.rb`, `broadcasts.rb`, `cgi_io.rb`, `db.rb`, `db_cruby.rb`, `importmap.rb`, `json.rb`, `sqlite_adapter.rb`) plus a `scaffold/` tree (Gemfile, inner Makefile, main.rb, Tailwind config) overlaid into every emitted Ruby/Spinel project, and a `test/` tree of target-specific test files |

## Framework runtime — `runtime/ruby/`

Ruby authoritative source for the Rails surface every emitted app
calls into:

```
runtime/ruby/
  active_record/        ApplicationRecord, validations, querying
  action_controller/    Base controller, Parameters
  action_view/          link_to, form_with, FormBuilder, pluralize, ...
  action_dispatch/      Routing helpers
  inflector.rb          camelize / pluralize / dasherize
```

Each `.rb` ships with a `.rbs` sidecar declaring the public typed
surface (see `analyze.md` on RBS-paired ingestion). The `.rbs` is
what makes the framework runtime typeable without annotating every
internal expression — only the public boundary commits to a type.

## How files reach the emitted project

Target primitives ship via Rust's `include_str!` at emitter compile
time:

```rust
const RUNTIME_SOURCE: &str = include_str!("../../runtime/rust/runtime.rs");
const DB_SOURCE: &str = include_str!("../../runtime/rust/db.rs");
// ...etc.
```

These strings are written verbatim into the generated project as
`src/runtime.rs`, `src/db.rs`, etc.

Framework runtime files (TS, eventually all targets) ship via the
same emit pipeline that compiles user apps — `runtime/ruby/active_record/`
is ingested with its RBS sidecar (`src/runtime_src.rs`), lowered, and
emitted into the generated project as `src/active_record_base.ts`
(etc.) by the same code path that compiles user controllers and
models. There is no separate `bin/build-runtime` binary; emission
runs inline as part of `cargo run --bin roundhouse -- --target <t>`
(or `--site` for the full archive matrix).

## Why hand-write the primitives?

1. **Framework integration is language-idiomatic.** axum middleware,
   Node's event loop, Plug's pipeline, Crystal's `HTTP::Handler` chain
   each look different in a way no higher-level IR captures.
2. **Editability.** When a primitive needs to grow (new middleware,
   new helper hook), editing a normal `.rs` / `.ts` file with IDE
   tooling is faster than editing a string inside a `format!`-driven
   emitter.

The tradeoff: emitters and primitives stay in lockstep. If
`view_helpers.rs` adds a function, the corresponding emitter (or, for
helpers, the `runtime/ruby/action_view/` source) has to learn to call
it. Snapshot tests + toolchain tests catch drift.

## Emitter ↔ runtime contract

For each target:

- **Emitter assumes** specific function names, signatures, and
  imports from the runtime (both layers).
- **Runtime guarantees** those functions exist and behave.
- **Snapshot tests** catch drift in emitter output.
- **Toolchain tests** catch drift in the runtime — if it no longer
  compiles, or if `cargo test` / `tsc --strict` / `crystal build`
  fails, the gate blocks the merge.

When adding a new helper: land the runtime change and the emitter
change in the same commit. Runtime that ships without emitter uptake
is dead code; emitter output that references a non-existent runtime
function is a compile failure in the generated project.

## Key files

| Directory | Role |
|-----------|------|
| `runtime/ruby/` | Framework runtime — Ruby source + RBS sidecars |
| `runtime/rust/` | Rust primitives |
| `runtime/typescript/` | TS primitives (framework runtime is emitter-generated from `runtime/ruby/`) |
| `runtime/crystal/` | Crystal primitives |
| `runtime/{go,python,elixir,spinel}/` | Sibling targets, partial |
| `src/emit/<target>.rs` | Emitter side that reads + embeds the runtime |
| `src/runtime_src.rs` | Framework-Ruby ingestion + transpile pipeline |

## Related docs

- [`emit.md`](emit.md) — the universal IR contract; the consumers of
  the runtime.
- [`analyze.md`](analyze.md) — RBS-paired typing of `runtime/ruby/`.
- [`verification.md`](verification.md) — toolchain tests that
  exercise runtime + emitted project end-to-end.
