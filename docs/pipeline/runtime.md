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
  active_record/                    transpiled per target
  action_controller/                  ↓
  action_view/                      runtime/typescript/active_record_base.ts
  action_dispatch/                  runtime/rust/...                          (planned)
  inflector.rb                      runtime/crystal/...                       (planned)
  ...                               etc.

runtime/<target>/                ← per-target primitives, hand-written
  http.<ext>                        DB connections, HTTP server,
  db.<ext>                          WebSocket plumbing, test harness —
  server.<ext>                      genuinely target-idiomatic glue
  test_support.<ext>                that no IR-level lowering captures
  cable.<ext>
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

Conventional file layout (modulo language extensions):

| File | Role |
|------|------|
| `runtime.<ext>` | Validation error type, model base trait/class shim |
| `db.<ext>` | DB connection lifecycle (open, with_conn borrow, test-mode in-memory) |
| `http.<ext>` | Controller-facing types: `Params`, `ActionContext`, `ActionResponse` |
| `server.<ext>` | Production HTTP server entry — listens on a port, dispatches through Router |
| `cable.<ext>` | Action Cable / WebSocket endpoint |
| `view_helpers.<ext>` | View helper shims that delegate into transpiled framework Ruby (where present) or implement helpers directly (legacy) |
| `test_support.<ext>` | TestClient + TestResponse + Rails-shaped assertions |

Not every target has every file yet. The current inventory:

| Target | Status |
|--------|--------|
| `runtime/rust/` | Full — `cable.rs`, `db.rs`, `http.rs`, `runtime.rs`, `server.rs`, `test_support.rs`, `view_helpers.rs` |
| `runtime/crystal/` | Full — same layout in `.cr` |
| `runtime/typescript/` | Full + framework-runtime files (transpiled from `runtime/ruby/`): `active_record_base.ts`, `action_controller_base.ts`, `parameters.ts`, `inflector.ts`, etc. `juntos.ts` is the legacy bundled stub being progressively replaced |
| `runtime/go/`, `runtime/python/`, `runtime/elixir/` | Partial; HTTP + Cable layers in flight |
| `runtime/spinel/` | Adapters + broadcast support (`broadcasts.rb`, `sqlite_adapter.rb`, `in_memory_adapter.rb`); reuses CRuby for the rest |

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
becomes `runtime/typescript/active_record_base.ts` via
`bin/build-runtime` (or equivalent), and the emitted project imports
the result.

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
| `runtime/typescript/` | TS primitives + transpiled framework runtime |
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
