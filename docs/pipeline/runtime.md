# Runtime

The `runtime/<target>/` directories hold hand-written per-target
support code that each emitted project links against. Every emitter
copies its matching runtime files verbatim into the generated
project.

**Source:** `runtime/rust/`, `runtime/typescript/`, `runtime/go/`,
`runtime/crystal/`, `runtime/elixir/`, `runtime/python/`.

## The architectural bet

The Phase-4 design decision: **most logic lives as IR-level lowerings
rendered inline by each target's emitter**, so the per-target runtime
shrinks to "adapters around target-specific primitives" rather than
being a mini-framework.

Concretely, that means:

- Validation evaluation is emitted inline (one `if Check... {
  errors.push(...) }` per lowered `Check`), not dispatched through a
  runtime validator. No validator class in the runtime.
- Route dispatch is emitted inline (flat match table), not interpreted
  from a runtime router DSL.
- SQL strings are computed at emit time and baked into generated
  code, not built at runtime from a query builder.
- Validation messages, form-field rendering, partial rendering — all
  emitted inline.

What stays in the runtime: the genuinely target-specific glue.
Opening a DB connection with rusqlite vs. better-sqlite3 vs. exqlite
is not something lowering can handle; starting an axum server vs. a
node HTTP server vs. a Plug pipeline isn't either.

## What every runtime provides

The file layout is consistent across targets (modulo language
conventions):

| File (Rust) | File (TS) | File (Go) | Role |
|-------------|-----------|-----------|------|
| `runtime.rs` | (embedded in `juntos.ts`) | `runtime.go` | Validation error struct, model base class / trait |
| `db.rs` | (embedded in `juntos.ts`) | `db.go` | DB connection lifecycle, `with_conn`-style borrow |
| `http.rs` | `http.ts` | `http.go` | Controller-facing types: `Params`, response helpers, re-exports of framework primitives |
| `server.rs` | `server.ts` | — | Production HTTP + Action Cable server (listens on a port, handles requests) |
| `view_helpers.rs` | `view_helpers.ts` | — | Rails view helpers: `link_to`, `button_to`, `form_with`, `FormBuilder`, `turbo_stream_from`, `dom_id`, `pluralize` |
| `test_support.rs` | `test_support.ts` | `test_support.go` | Helpers used by emitted controller tests |
| — | `juntos.ts` | — | TS-only: bundles the Juntos-shape runtime (model base, validations, persistence) |

Not every target has every file yet. Crystal, Elixir, Python, and Go
still have runtime glue in flight for the server-level pieces (HTTP +
Action Cable).

## How files reach the emitted project

The Rust emitter uses Rust's `include_str!` at emitter compile time:

```rust
const RUNTIME_SOURCE: &str = include_str!("../../runtime/rust/runtime.rs");
const DB_SOURCE: &str = include_str!("../../runtime/rust/db.rs");
// ...etc.
```

These strings are written verbatim into the generated project as
`src/runtime.rs`, `src/db.rs`, etc. Editing a runtime file doesn't
require rebuilding — it's just a `.rs` file that gets re-read at
cargo-compile time for the roundhouse binary itself.

Other emitters follow the same pattern with their language's
equivalent (TypeScript emitter reads the `.ts` files and writes them
to the generated project's `runtime/` dir; Go emitter reads `.go`
files and writes them to the `app/` package; etc.).

## Rust runtime walkthrough

### `runtime.rs`

Tiny. Just `ValidationError { field, message }` today. Generated
models hold their errors as `Vec<ValidationError>`; the emitter
produces `errors.push(ValidationError::new("title", "can't be blank"))`
inline at each check site. No runtime validator machinery needed.

### `db.rs`

Owns the process's SQLite connection(s). Two entry points:

- `setup_test_db(schema)` — thread-local `:memory:` connection. Each
  test installs a fresh DB so prior-test state doesn't bleed.
- `open_production_db(path, schema)` — file-backed connection in a
  process-wide `Mutex<Option<Connection>>`. `main.rs` calls this on
  server boot.

`with_conn(|c| ...)` borrows whichever slot is populated — test
thread-local first, then process mutex. That lets the same generated
model code work in both test and production without conditional
branches.

### `http.rs`

Controller-facing types + re-exports:

- `Params` — wrapper over form-extracted bracketed params
  (`article[title]` style).
- `ViewCtx` — per-request flash/notice/alert context passed through
  to views.
- Re-exports of `axum::response::{Html, IntoResponse, Redirect,
  Response}` under `crate::http::*` so generated actions have a
  single import path.

### `server.rs`

The production entrypoint. `main.rs` (generated) calls `start(router,
opts)`; the server runtime opens the DB, applies schema, installs
middleware, and runs axum.

Middleware stack (outer → inner):

- `layout_wrap` — wraps HTML responses in the full document shell
  (Tailwind + importmap + Action Cable meta). Mirrors the TS
  runtime's `renderLayout`.
- `method_override` — Rails forms POST `_method=patch|put|delete`; the
  runtime reads the form body, rewrites the request method, and
  re-injects the body so downstream `axum::Form` extractors still
  work.

Action Cable / WebSocket support is stubbed today — the
`turbo_stream_from` helper emits a valid subscription tag, but the
server doesn't yet open the `/cable` endpoint or broadcast updates.
Turbo attempts to connect and fails quietly; navigation + form-submit
flows still work.

### `view_helpers.rs`

The Rails view helpers: `link_to`, `button_to`, `form_wrap`,
`FormBuilder` methods, `turbo_stream_from`, `dom_id`, `pluralize`,
plus the `RenderCtx` carrying layout slots. Mirrors the TS runtime
in signature.

Implementation note: `FormBuilder`'s per-field methods take the
current value as an explicit `&str` arg rather than reading off a
trait-object record. The emit side knows the record + field and
produces the direct field access inline — that keeps the runtime
free of dynamic-field dispatch (which Rust would otherwise need a
trait + derive to provide).

## TypeScript runtime walkthrough

### `juntos.ts`

The TS emitter targets [Juntos](https://www.ruby2js.com/docs/juntos/)
— a ruby2js extension. `juntos.ts` in the roundhouse tree is a
**minimal Juntos-shape stub** providing the subset the emitted
project needs: typed model surface, validation primitives, a
better-sqlite3-backed persistence layer keyed on per-subclass
metadata (`table_name`, `columns`, `belongsToChecks`,
`dependentChildren`). Real Juntos takes over in production via
`tsconfig` path mapping.

Also handles DB connection lifecycle (`installDb`, `setupTestDb`,
`conn`), Router class + `setBroadcaster` hook for Action Cable.

### `server.ts`

The emitted `main.ts` imports `startServer` from here and hands it
the Router's match function. `startServer`:

1. Opens a file-backed better-sqlite3 database.
2. Runs schema DDL (from the generated `schema_sql.ts`).
3. Installs the Action Cable broadcaster on `ApplicationRecord`.
4. Starts an HTTP listener that routes requests through
   `Router.match` → ActionContext → controller action →
   ActionResponse → HTTP response.
5. Upgrades WebSocket connections on `/cable` into Action Cable
   clients with `actioncable-v1-json` subprotocol, pings every 3s,
   and broadcasts turbo-stream fragments to subscribed channels.

Based on railcar's TS `app.ts` pattern, adapted to roundhouse's
emitted `Router`/`ActionContext`/`ActionResponse` shapes.

### `view_helpers.ts`

Same Rails view helpers as the Rust version, rendered in TypeScript.
Signature parity is intentional — the cross-runtime comparator
(`tools/compare`) expects rendered HTML to match byte-for-byte
modulo CSRF tokens and asset fingerprints.

## Why hand-written rather than generated?

Two reasons:

1. **Framework integration is language-idiomatic.** Wiring axum
   middleware, node's `http` module event loop, and Plug's pipeline
   DSL each looks different in a way that isn't captured by a
   higher-level IR. Trying to generate these would either constrain
   the output to a lowest-common-denominator shape or require
   per-target templates that are just as complex as the
   hand-written equivalents.

2. **Editability.** When a runtime needs to grow (add a new route
   middleware, support a new view helper), editing a normal `.rs` /
   `.ts` file with its IDE tooling is faster than editing a string
   inside a `format!`-driven emitter.

The tradeoff: emitters and runtimes have to stay in lockstep. If
`view_helpers.rs` adds a function, `src/emit/rust.rs` has to learn
to call it from the corresponding view render. That coupling is
enforced by snapshot tests + toolchain tests.

## The emitter ↔ runtime contract

For each target:

- **Emitter assumes** specific function names, signatures, and
  imports from the runtime.
- **Runtime guarantees** those functions exist and behave.
- **Snapshot tests** catch drift in emitter output.
- **Toolchain tests** catch drift in the runtime (if it no longer
  compiles, or if `cargo test` / `tsc --strict` / etc. fails).

When adding a new helper: land the runtime change and the emitter
change in the same commit. A runtime file that ships without
emitter uptake is dead code; emitter output that references a
non-existent runtime function is a compile failure in the generated
project.

## Key files

| File | Role |
|------|------|
| `runtime/rust/` | Rust runtime: runtime.rs, db.rs, http.rs, server.rs, view_helpers.rs, test_support.rs |
| `runtime/typescript/` | TS runtime: juntos.ts, server.ts, http.ts, view_helpers.ts, test_support.ts |
| `runtime/{go,crystal,elixir,python}/` | Sibling targets, analogous layout |
| `src/emit/rust.rs`, etc. | Emitter side that reads + embeds the runtime |

## Related docs

- [`emit.md`](emit.md) — how emitters consume the runtime.
- [`verification.md`](verification.md) — toolchain tests that exercise
  the runtime end-to-end.
